// SPDX-License-Identifier: CC0-1.0

//! `ListCustomerAgreements`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones the operation's model states:
//!
//! * the answer is COMPUTED from the persisted customer-agreement set, so an agreement
//!   written into the store is reported and one removed from it is not, and two calls
//!   report the same records;
//! * the operation "List[s] active customer-agreements applicable to calling identity",
//!   so an agreement the store holds in a terminated `CustomerAgreementState` is withheld
//!   while the ACTIVE ones are reported, each projected from its stored record;
//! * `@paginated(items: customerAgreements)`: a page is cut at `maxResults`, a `nextToken`
//!   arrives exactly when agreements remain, and it resumes where the previous page
//!   ended — so walking the pages at ANY admissible page size enumerates the whole set
//!   once; the token is an OPAQUE cursor rather than an agreement id, and one this
//!   service never issued is refused;
//! * `customerAgreements` is `@required`, so it is serialized even when nothing is
//!   active.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_artifact_artifact::state::{
    customer_agreements_page_token, service_state, ComAmazonawsArtifactArtifactState,
    CustomerAgreement, PageTokenKey,
};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The bounds `MaxResultsAttribute` declares (`@range(min: 1, max: 300)`), spelled out
/// here rather than imported from the crate so a wrong bound in `src/state.rs` fails.
const MAX_RESULTS_FLOOR: i32 = 1;
/// See [`MAX_RESULTS_FLOOR`].
const MAX_RESULTS_CEILING: i32 = 300;

/// The region the default test request is signed for.
const REGION: &str = "eu-west-1";

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// A credential scope for `region`, in the shape SigV4 declares. Its signing name is
/// what routes the request to the artifact service, and its region is what a
/// customer-agreement ARN is built for.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One ACTIVE customer-agreement the emulator publishes for its account.
///
/// The API has no operation that creates, accepts or terminates a customer-agreement, so
/// these rows are the account's established agreements. They are spelled out here,
/// independently of the seed in `src/state.rs`, so a changed or fabricated set fails this
/// suite rather than quietly redefining what it asserts, and they differ along everything
/// the rules generalise over: `AgreementType`, effective date, and how many acceptance and
/// termination terms an agreement carries.
struct Expected {
    id: &'static str,
    name: &'static str,
    /// The AWS-published agreement the customer-agreement's `agreementArn` names.
    agreement: &'static str,
    /// One of `AgreementType`'s values (`CUSTOM` | `DEFAULT` | `MODIFIED`).
    agreement_type: &'static str,
    /// `effectiveStart`, as the wire carries it: `TimestampAttribute` is
    /// `@timestampFormat("date-time")`, so an RFC 3339 date-time STRING — never the
    /// restJson1 default of epoch seconds.
    effective_start: &'static str,
    acceptance_terms: usize,
    terminate_terms: usize,
}

/// The ACTIVE agreements, in the ascending customer-agreement-id order the operation
/// reports them in.
const ACTIVE: &[Expected] = &[
    Expected {
        id: "customer-agreement-baaagreement0001",
        name: "AWS Business Associate Addendum",
        agreement: "AWS-BAA",
        agreement_type: "DEFAULT",
        // 2024-01-01T00:00:00Z
        effective_start: "2024-01-01T00:00:00Z",
        acceptance_terms: 2,
        terminate_terms: 1,
    },
    Expected {
        id: "customer-agreement-gdprsccagreement",
        name: "AWS GDPR Data Processing Addendum",
        agreement: "AWS-GDPR-DPA",
        agreement_type: "MODIFIED",
        // 2024-07-01T00:00:00Z
        effective_start: "2024-07-01T00:00:00Z",
        acceptance_terms: 3,
        terminate_terms: 2,
    },
    Expected {
        id: "customer-agreement-ndaagreement0001",
        name: "AWS Nondisclosure Agreement",
        agreement: "AWS-NDA",
        agreement_type: "CUSTOM",
        // 2025-01-01T00:00:00Z
        effective_start: "2025-01-01T00:00:00Z",
        acceptance_terms: 1,
        terminate_terms: 1,
    },
];

/// The agreements the store holds in a terminated state, with that state. They are
/// persisted — and withheld from the answer, because the operation reports the ACTIVE
/// ones. One row per terminated `CustomerAgreementState` value.
const TERMINATED: &[(&str, &str)] = &[
    (
        "customer-agreement-cjisagreement001",
        "CUSTOMER_TERMINATED",
    ),
    ("customer-agreement-pcidssagreement1", "AWS_TERMINATED"),
];

/// `CustomerAgreementIdAttribute`'s `^customer-agreement-[a-zA-Z0-9]{16}$`, checked here
/// rather than through the crate's own predicate so a wrong predicate cannot make this
/// test pass.
fn matches_customer_agreement_id_pattern(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("customer-agreement-") else {
        return false;
    };
    suffix.len() == 16 && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// One emulator over `engine`, so a test can inspect and write the same store the
/// handler reads.
fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The secret this emulator's store seals its own pagination cursors under: what a case
/// needing an AUTHENTIC cursor mints one with, through the very codec the handler mints
/// from. A caller holds no such secret, which is why a cursor it assembles is refused.
fn page_token_key(engine: &engine::InMemoryEngine) -> PageTokenKey {
    service_state(engine)
        .expect("the in-memory engine carries this service's store")
        .lock()
        .page_token_key()
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `customerAgreements` list the operation returns, or `None` when the member is
    /// missing altogether — which a `@required` member never may be on a success.
    fn agreements(&self) -> Option<&Vec<Value>> {
        self.body["customerAgreements"].as_array()
    }

    /// The reported ids, in the order they arrived.
    fn ids(&self) -> Vec<String> {
        self.agreements()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `nextToken` the answer carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }
}

async fn list_signed(router: &MultiServiceRouter, query: &str, authorization: &str) -> Answer {
    let uri = format!("/v1/customer-agreement/list{query}");
    let request = ::http::Request::builder()
        .method("GET")
        .uri(&uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", authorization)
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "GET {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    list_signed(router, query, &signed_for(REGION)).await
}

/// The ARN form a customer-agreement resource carries: account-owned, in the region the
/// request was signed for.
fn agreement_resource_arn(region: &str, id: &str) -> String {
    format!(
        "arn:aws:artifact:{region}:{account}:customer-agreement/{id}",
        account = engine::emulator_account_id()
    )
}

/// The ARN form the AWS-published agreement resource carries: neither account- nor
/// region-scoped.
fn published_agreement_arn(agreement: &str) -> String {
    format!("arn:aws:artifact:::agreement/{agreement}")
}

/// Walk every page at `page_size`, returning the concatenated summaries.
///
/// Asserts the token rule as it goes: a page is never longer than the requested size, and
/// a `nextToken` only ever arrives with a full page.
async fn walk(router: &MultiServiceRouter, page_size: i32, total: usize) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=total {
        let query = match token.as_deref() {
            None => format!("?maxResults={page_size}"),
            Some(value) => format!("?maxResults={page_size}&nextToken={value}"),
        };
        let answer = list(router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={page_size} page {page}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = answer
            .agreements()
            .unwrap_or_else(|| {
                panic!(
                    "case maxResults={page_size} page {page}: customerAgreements is required, \
                     got {:?}",
                    answer.body
                )
            })
            .clone();
        assert!(
            items.len() <= usize::try_from(page_size).expect("a positive page size"),
            "case maxResults={page_size} page {page}: a page must hold at most {page_size} \
             agreements, got {}",
            items.len()
        );
        collected.extend(items.iter().cloned());
        match answer.next_token() {
            None => return collected,
            Some(next) => {
                assert_eq!(
                    items.len(),
                    usize::try_from(page_size).expect("a positive page size"),
                    "case maxResults={page_size} page {page}: a nextToken must only arrive with \
                     a full page, got {} items",
                    items.len()
                );
                token = Some(next);
            }
        }
    }
    panic!("case maxResults={page_size}: pagination did not terminate over {total} agreements");
}

#[tokio::test]
async fn every_active_agreement_is_projected_from_the_persisted_record() {
    let router = emulator();

    let first = list(&router, "").await;
    assert_eq!(
        first.status, 200,
        "expected HTTP 200 from ListCustomerAgreements, got {} with {:?}",
        first.status, first.body
    );
    let second = list(&router, "").await;
    assert_eq!(
        first.agreements(),
        second.agreements(),
        "two successive calls must report the same records; first {:?}, second {:?}",
        first.body,
        second.body
    );

    let items = first
        .agreements()
        .unwrap_or_else(|| panic!("customerAgreements is required, got {:?}", first.body));
    assert_eq!(
        items.len(),
        ACTIVE.len(),
        "expected every active agreement, got {:?}",
        first.body
    );
    assert_eq!(
        first.next_token(),
        None,
        "a page holding every agreement must carry no nextToken, got {:?}",
        first.body
    );

    for (expected, actual) in ACTIVE.iter().zip(items) {
        let case = expected.id;
        assert_eq!(
            actual["id"].as_str(),
            Some(expected.id),
            "case {case:?}: expected id {:?}, got {actual:?}",
            expected.id
        );
        assert!(
            matches_customer_agreement_id_pattern(actual["id"].as_str().unwrap_or_default()),
            "case {case:?}: id must match ^customer-agreement-[a-zA-Z0-9]{{16}}$, got {:?}",
            actual["id"]
        );
        assert_eq!(
            actual["name"].as_str(),
            Some(expected.name),
            "case {case:?}: expected name {:?}, got {actual:?}",
            expected.name
        );
        assert_eq!(
            actual["type"].as_str(),
            Some(expected.agreement_type),
            "case {case:?}: expected type {:?}, got {actual:?}",
            expected.agreement_type
        );
        // "List active customer-agreements": every reported agreement is ACTIVE, and an
        // ACTIVE one was never terminated, so it carries no termination timestamp.
        assert_eq!(
            actual["state"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: a reported agreement must be ACTIVE, got {actual:?}"
        );
        assert!(
            actual["effectiveEnd"].is_null(),
            "case {case:?}: an active agreement carries no effectiveEnd, got {:?}",
            actual["effectiveEnd"]
        );
        assert_eq!(
            actual["effectiveStart"].as_str(),
            Some(expected.effective_start),
            "case {case:?}: expected effectiveStart {:?}, got {actual:?}",
            expected.effective_start
        );
        assert_eq!(
            actual["arn"].as_str(),
            Some(agreement_resource_arn(REGION, expected.id).as_str()),
            "case {case:?}: expected the account-owned ARN {:?}, got {actual:?}",
            agreement_resource_arn(REGION, expected.id)
        );
        assert_eq!(
            actual["agreementArn"].as_str(),
            Some(published_agreement_arn(expected.agreement).as_str()),
            "case {case:?}: expected agreementArn {:?}, got {actual:?}",
            published_agreement_arn(expected.agreement)
        );
        assert_eq!(
            actual["awsAccountId"].as_str(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected awsAccountId {:?}, got {actual:?}",
            engine::emulator_account_id()
        );
        assert!(
            actual["organizationArn"].is_null(),
            "case {case:?}: no organization owns the agreement, got {:?}",
            actual["organizationArn"]
        );
        assert!(
            actual["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty() && !text.contains(['<', '>'])),
            "case {case:?}: description must match ^[^<>]*$ within 1..2048, got {:?}",
            actual["description"]
        );
        // `AgreementTerms` is `@length(max: 10)`, so a stored term list is reported as it
        // stands and never exceeds the bound.
        for (member, count) in [
            ("acceptanceTerms", expected.acceptance_terms),
            ("terminateTerms", expected.terminate_terms),
        ] {
            let terms = actual[member]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: {member} must be a list, got {actual:?}"));
            assert_eq!(
                terms.len(),
                count,
                "case {case:?}: expected {count} {member}, got {terms:?}"
            );
            assert!(
                terms.len() <= 10
                    && terms.iter().all(|term| term
                        .as_str()
                        .is_some_and(|text| !text.is_empty() && !text.contains(['<', '>']))),
                "case {case:?}: every {member} entry must match ^[^<>]*$ within 1..2048, got \
                 {terms:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_terminated_agreement_is_persisted_and_withheld() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    // The store really does hold the terminated agreements, so withholding them is a
    // state rule the handler applies and not an empty set.
    {
        let state = engine.state::<ComAmazonawsArtifactArtifactState>();
        let store = state.lock();
        assert_eq!(
            store.customer_agreements.len(),
            ACTIVE.len() + TERMINATED.len(),
            "the store must hold every seeded agreement, got {:?}",
            store.customer_agreements.keys().collect::<Vec<_>>()
        );
        for (id, expected_state) in TERMINATED {
            let held = store
                .customer_agreements
                .get(*id)
                .unwrap_or_else(|| panic!("case {id:?}: the store must hold the agreement"));
            assert_eq!(
                held.state, *expected_state,
                "case {id:?}: expected state {expected_state:?}, got {:?}",
                held.state
            );
        }
    }

    let answer = list(&router, "").await;
    let reported = answer.ids();
    for (id, expected_state) in TERMINATED {
        assert!(
            !reported.iter().any(|seen| seen == id),
            "case {id:?}: an agreement in {expected_state} must not be reported, got {reported:?}"
        );
    }
    assert_eq!(
        reported,
        ACTIVE.iter().map(|row| row.id.to_string()).collect::<Vec<_>>(),
        "expected exactly the active agreements, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn the_answer_follows_the_store_a_write_changes() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    // Two records written straight into the store, differing in exactly the member the
    // state rule keys on. Neither is in the seed, so an answer that reports the added
    // ACTIVE one and withholds the added terminated one is computed from state.
    let added_active = "customer-agreement-injectedbyatest1";
    let added_terminated = "customer-agreement-injectedbyatest2";
    let effective_start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600);
    for (id, state_value) in [(added_active, "ACTIVE"), (added_terminated, "AWS_TERMINATED")] {
        let record = CustomerAgreement {
            id: id.to_string(),
            name: "Test Addendum".to_string(),
            description: "An agreement written directly into the store".to_string(),
            agreement_id: "AWS-TEST".to_string(),
            agreement_type: "CUSTOM".to_string(),
            state: state_value.to_string(),
            effective_start,
            effective_end: None,
            acceptance_terms: vec!["Acceptance by the test".to_string()],
            terminate_terms: vec!["Termination by the test".to_string()],
        };
        engine
            .state::<ComAmazonawsArtifactArtifactState>()
            .lock()
            .customer_agreements
            .insert(id.to_string(), record);
    }

    let answer = list(&router, "").await;
    let reported = answer.ids();
    assert!(
        reported.iter().any(|id| id == added_active),
        "case {added_active:?}: an ACTIVE record written into the store must be reported, got \
         {reported:?}"
    );
    assert!(
        !reported.iter().any(|id| id == added_terminated),
        "case {added_terminated:?}: a terminated record must not be reported, got {reported:?}"
    );
    let mut sorted = reported.clone();
    sorted.sort();
    assert_eq!(
        reported, sorted,
        "agreements must be reported in ascending id order, got {reported:?}"
    );
    let added = answer
        .agreements()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["id"].as_str() == Some(added_active))
        })
        .unwrap_or_else(|| panic!("the added agreement must be reported, got {:?}", answer.body))
        .clone();
    assert_eq!(
        added["arn"].as_str(),
        Some(agreement_resource_arn(REGION, added_active).as_str()),
        "case {added_active:?}: the ARN must be built for the stored id, got {added:?}"
    );
    assert_eq!(
        added["agreementArn"].as_str(),
        Some(published_agreement_arn("AWS-TEST").as_str()),
        "case {added_active:?}: the agreementArn must name the stored agreement, got {added:?}"
    );

    // And with every agreement gone, the `@required` list is still serialized — empty.
    engine
        .state::<ComAmazonawsArtifactArtifactState>()
        .lock()
        .customer_agreements
        .clear();
    let empty = list(&router, "").await;
    assert_eq!(
        empty.status, 200,
        "an account with no agreement must still answer 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.agreements().map(Vec::len),
        Some(0),
        "customerAgreements is required, so an empty set is still serialized, got {:?}",
        empty.body
    );
    assert_eq!(
        empty.next_token(),
        None,
        "an empty page must carry no nextToken, got {:?}",
        empty.body
    );
}

#[tokio::test]
async fn pagination_enumerates_the_set_once_at_every_admissible_page_size() {
    let router = emulator();
    let whole = list(&router, &format!("?maxResults={MAX_RESULTS_CEILING}")).await;
    let expected = whole
        .agreements()
        .unwrap_or_else(|| panic!("customerAgreements is required, got {:?}", whole.body))
        .clone();
    assert_eq!(
        expected.len(),
        ACTIVE.len(),
        "the ceiling page must hold every active agreement, got {:?}",
        whole.body
    );

    // Every page size the model admits that can cut this set, plus both published bounds.
    let mut sizes: Vec<i32> = (MAX_RESULTS_FLOOR..=i32::try_from(ACTIVE.len() + 1).unwrap())
        .chain([MAX_RESULTS_CEILING])
        .collect();
    sizes.dedup();
    for page_size in sizes {
        let walked = walk(&router, page_size, ACTIVE.len()).await;
        assert_eq!(
            walked, expected,
            "case maxResults={page_size}: walking the pages must enumerate the whole set once, \
             got {walked:?}"
        );
    }

    // Absent `maxResults` serves the largest page the model allows, so the whole set.
    let defaulted = list(&router, "").await;
    assert_eq!(
        defaulted.agreements().cloned(),
        Some(expected),
        "an absent maxResults must serve the whole set, got {:?}",
        defaulted.body
    );
}

#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    // `MaxResultsAttribute` is `@range(min: 1, max: 300)`: each bound is served, and one
    // step past either is refused at the boundary.
    for accepted in [MAX_RESULTS_FLOOR, MAX_RESULTS_CEILING] {
        let answer = list(&router, &format!("?maxResults={accepted}")).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={accepted}: a bound of the modelled range must be served, got {} \
             with {:?}",
            answer.status, answer.body
        );
    }
    for refused_value in [MAX_RESULTS_FLOOR - 1, MAX_RESULTS_CEILING + 1] {
        let refused = list(&router, &format!("?maxResults={refused_value}")).await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={refused_value}: a value outside 1..300 must be refused, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case maxResults={refused_value}: expected ValidationException, got {:?}",
            refused.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let key = page_token_key(&engine);
    // A `nextToken` is an OPAQUE cursor this operation cut, so every row here is a value
    // that is not one: a made-up string, a token of another operation, an id whose suffix
    // is the wrong length or carries characters the pattern excludes, and — the case this
    // operation used to serve — a BARE customer-agreement id, which is a resource
    // identifier rather than a cursor and so was never issued as a token.
    for token in [
        "not-a-token",
        "0",
        "customer-agreement-",
        "customer-agreement-short",
        "customer-agreement-waytoolongtobeanid",
        "customer-agreement-not_alphanumeric",
        "report-cloudsecalliance",
        "compliance-inquiry-abcdefghij123456",
        ACTIVE[0].id,
        ACTIVE[1].id,
    ] {
        let refused = list(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case {token:?}: an unissued nextToken must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }

    // The token this operation DOES issue carries none of the set's identifiers in the
    // clear, and stays inside `NextTokenAttribute`'s `@length(1..2048)`.
    let issued = list(&router, "?maxResults=1")
        .await
        .next_token()
        .expect("a set of several active agreements must page");
    assert!(
        !issued.is_empty() && issued.len() <= 2048,
        "case \"the issued token\": NextTokenAttribute is @length(1..2048), got {} bytes",
        issued.len()
    );
    for row in ACTIVE {
        assert!(
            issued != row.id && !issued.contains(row.id),
            "case \"the issued token\": a pagination token must not be or carry the \
             identifier {:?}, got {issued:?}",
            row.id
        );
    }

    // And the token taken from an ACTUAL page resumes at the agreement after that page,
    // so the cursor rows below cannot pass on a symmetric encode/decode defect.
    let resumed = list(&router, &format!("?nextToken={issued}")).await;
    assert_eq!(
        resumed.status, 200,
        "case \"the issued token\": a token this operation cut must be served, got {} with {:?}",
        resumed.status, resumed.body
    );
    assert_eq!(
        resumed.ids(),
        ACTIVE
            .iter()
            .skip(1)
            .map(|row| row.id.to_string())
            .collect::<Vec<_>>(),
        "case \"the issued token\": expected the agreements after the first page, got {:?}",
        resumed.body
    );

    // A cursor this operation cut resumes the listing at the POSITION it carries, so the
    // page after a cursor is exactly the tail of the reported order behind it — including
    // one position past the last agreement, which serves an empty page rather than
    // refusing.
    for position in 0..=ACTIVE.len() {
        let expected: Vec<String> = ACTIVE
            .iter()
            .skip(position)
            .map(|row| row.id.to_string())
            .collect();
        let token = customer_agreements_page_token(&key, position);
        let answer = list(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            answer.status, 200,
            "case position={position}: a cursor this operation cut must be served, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.ids(),
            expected,
            "case position={position}: expected the agreements after the cursor, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn an_agreement_arn_names_the_signed_region_and_the_owning_account() {
    // The rule generalises over the signing region, so the rows are regions: the usual
    // shapes, a three-segment one, and a scope whose region field is not shaped like a
    // region at all — which falls back rather than letting a header shape an ARN.
    for (case, region, expected_region) in [
        ("us-east-1", "us-east-1", FALLBACK_REGION),
        ("eu-west-1", REGION, REGION),
        ("ap-southeast-3", "ap-southeast-3", "ap-southeast-3"),
        ("not_a_region", "not_a_region", FALLBACK_REGION),
        ("an empty region", "", FALLBACK_REGION),
    ] {
        let router = emulator();
        let answer = list_signed(&router, "", &signed_for(region)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = answer
            .agreements()
            .unwrap_or_else(|| panic!("case {case:?}: customerAgreements is required"));
        assert_eq!(
            items.len(),
            ACTIVE.len(),
            "case {case:?}: the signing region must not change which agreements are reported, \
             got {:?}",
            answer.body
        );
        for item in items {
            let id = item["id"].as_str().unwrap_or_default();
            assert_eq!(
                item["arn"].as_str(),
                Some(agreement_resource_arn(expected_region, id).as_str()),
                "case {case:?}: expected the ARN {:?}, got {item:?}",
                agreement_resource_arn(expected_region, id)
            );
            // The published agreement is neither account- nor region-scoped, so its ARN
            // does not move with the request.
            assert!(
                item["agreementArn"]
                    .as_str()
                    .is_some_and(|arn| arn.starts_with("arn:aws:artifact:::agreement/")),
                "case {case:?}: the published agreement ARN carries no account or region, got \
                 {item:?}"
            );
        }
    }
}
