// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the pca-connector-ad directory-registration slice:
//! `CreateDirectoryRegistration`, `GetDirectoryRegistration` and
//! `ListDirectoryRegistrations`.
//!
//! Every case drives the real wire path through [`super::harness`] — the same
//! `MultiServiceRouter::dispatch` the production front door uses — so routing,
//! `@httpLabel` binding, constraint validation, the handler and serialization are
//! all exercised. The only cases that reach into the state struct are the ones
//! about a clock-owed move, and they move the DUE INSTANT only — never the status,
//! which the handlers still compute from that record.
//!
//! No service that creates Active Directory directories is mounted here, so the
//! account holds none: a registration of this suite settles on the failure its
//! unheld directory earns it. The ACTIVE branch of that rule, and the removal a
//! delete owes, are in [`super::directory_registration_authorization`].

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pcaconnectorad_pcaconnectorad as pcad;
// The store bound the handler refuses past, imported from the crate so this suite
// cannot drift from the cap the handler enforces.
use pcad::state::MAX_DIRECTORY_REGISTRATIONS;

use super::harness::{call, encode_label, router_with_engine as router, ACCOUNT, REGION};

/// Far enough in the future that no case outlives it, so every call in a case that
/// holds a transition observes the transient status.
const NOT_DUE_FOR: Duration = Duration::from_secs(3600);

/// Any past instant is due, so the next call settles the transition.
const ALREADY_DUE: Duration = Duration::from_secs(1);

/// Directory ids spanning the modelled `DirectoryId` pattern `^d-[0-9a-f]{10}$`
/// (`smithy_query(query="shape",shape="DirectoryId")`): digits only, hex letters
/// only, the two extremes of the value range, and mixtures — the dimension the
/// rule under test generalises over, since the registration's ARN is built from
/// this id.
const DIRECTORY_IDS: &[&str] = &[
    "d-0123456789",
    "d-abcdefabcd",
    "d-0000000000",
    "d-ffffffffff",
    "d-1a2b3c4d5e",
    "d-deadbeef00",
];

/// `CreateDirectoryRegistration` for `directory_id`, answering the ARN it minted.
pub(crate) async fn create_registration(
    router: &MultiServiceRouter,
    directory_id: &str,
) -> String {
    let (status, _, parsed) = call(
        router,
        "POST",
        "/directoryRegistrations",
        Some(&json!({ "DirectoryId": directory_id })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {directory_id}: CreateDirectoryRegistration must answer the modelled 202, \
         got {status} / {parsed}"
    );
    parsed["DirectoryRegistrationArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {directory_id}: the create answered no DirectoryRegistrationArn: {parsed}")
        })
        .to_string()
}

pub(crate) async fn get_registration(
    router: &MultiServiceRouter,
    arn: &str,
) -> (u16, String, Value) {
    call(
        router,
        "GET",
        &format!("/directoryRegistrations/{}", encode_label(arn)),
        None,
    )
    .await
}

/// One `ListDirectoryRegistrations` page.
pub(crate) async fn list_page(
    router: &MultiServiceRouter,
    max_results: Option<usize>,
    next_token: Option<&str>,
) -> (u16, String, Value) {
    let mut query = String::from("/directoryRegistrations");
    let mut separator = '?';
    if let Some(max_results) = max_results {
        query.push(separator);
        query.push_str(&format!("MaxResults={max_results}"));
        separator = '&';
    }
    if let Some(token) = next_token {
        query.push(separator);
        query.push_str(&format!("NextToken={}", encode_label(token)));
    }
    call(router, "GET", &query, None).await
}

/// The `Arn` of every entry a list page carries, in the order served.
fn page_arns(page: &Value) -> Vec<String> {
    page["DirectoryRegistrations"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|summary| summary["Arn"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The ARN prefix every directory registration of this Region carries.
pub(crate) fn registration_arn_prefix() -> String {
    format!("arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:directory-registration/")
}

/// Move the instant the clock owes the registration `arn` its move out of
/// `CREATING` to `due_at`, through the service's own state.
///
/// Strictly a clock move: it rewrites only the due instant, never the status, the
/// terminal status it settles on or the timestamps — the handlers still compute
/// those from this same record.
pub(crate) fn set_settle_due(engine: &engine::InMemoryEngine, arn: &str, due_at: SystemTime) {
    let shared = pcad::state::service_state(engine).expect("the service's own state");
    let mut store = shared.lock();
    let record = store
        .directory_registrations
        .get_mut(arn)
        .unwrap_or_else(|| panic!("no directory registration is stored under {arn:?}"));
    record.settles_at = due_at;
}

#[tokio::test]
async fn create_directory_registration_mints_an_arn_naming_the_directory_and_get_reads_it_back() {
    let (router, engine) = router();

    for directory_id in DIRECTORY_IDS {
        let arn = create_registration(&router, directory_id).await;
        assert_eq!(
            arn,
            format!("{}{directory_id}", registration_arn_prefix()),
            "case {directory_id}: the ARN must name this directory under the request's \
             Region and the emulator account, got {arn:?}"
        );

        // Hold the clock-owed transition so this case reads the status the 202
        // reported rather than racing the activation span.
        set_settle_due(&engine, &arn, SystemTime::now() + NOT_DUE_FOR);

        let (status, _, parsed) = get_registration(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {directory_id}: GetDirectoryRegistration must answer 200, got {status} / {parsed}"
        );
        let registration = &parsed["DirectoryRegistration"];
        assert_eq!(
            registration["Arn"].as_str(),
            Some(arn.as_str()),
            "case {directory_id}: the read must report the ARN the create minted, got {registration}"
        );
        assert_eq!(
            registration["DirectoryId"].as_str(),
            Some(*directory_id),
            "case {directory_id}: the read must report the directory id the create was given, \
             got {registration}"
        );
        assert_eq!(
            registration["Status"].as_str(),
            Some("CREATING"),
            "case {directory_id}: a registration whose activation is not due yet is CREATING, \
             got {registration}"
        );
        assert!(
            registration["StatusReason"].is_null(),
            "case {directory_id}: a registration that did not fail carries no StatusReason, \
             got {registration}"
        );
        assert!(
            registration["CreatedAt"].is_number() && registration["UpdatedAt"].is_number(),
            "case {directory_id}: CreatedAt/UpdatedAt must be set, got {registration}"
        );
    }
}

#[tokio::test]
async fn get_directory_registration_on_an_arn_that_was_never_created_is_resource_not_found() {
    let (router, _engine) = router();
    create_registration(&router, DIRECTORY_IDS[0]).await;

    // Every ARN here satisfies the modelled `DirectoryRegistrationArn` pattern, so
    // the refusal comes from the handler and not from the boundary validator: an
    // unregistered directory of this Region, one of another Region, and one of
    // another partition.
    let unknown = [
        format!("{}d-5555555555", registration_arn_prefix()),
        format!("arn:aws:pca-connector-ad:eu-west-1:{ACCOUNT}:directory-registration/d-aaaabbbbcc"),
        format!(
            "arn:aws-us-gov:pca-connector-ad:{REGION}:{ACCOUNT}:directory-registration/d-99887766ff"
        ),
    ];
    for arn in unknown {
        let (status, code, parsed) = get_registration(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {arn}: a registration that does not exist is a 404, got {status} / {parsed}"
        );
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {arn}: expected ResourceNotFoundException, got {code:?} / {parsed}"
        );
        assert_eq!(
            parsed["Message"].as_str(),
            Some(format!("DirectoryRegistration was not found: {arn}.").as_str()),
            "case {arn}: the refusal must read as this service words a missing resource, \
             got {parsed}"
        );
        assert_eq!(
            parsed["ResourceId"].as_str(),
            Some(arn.as_str()),
            "case {arn}: the refusal must name the identifier as supplied, got {parsed}"
        );
        assert_eq!(
            parsed["ResourceType"].as_str(),
            Some("DirectoryRegistration"),
            "case {arn}: the refusal must name the resource type, got {parsed}"
        );
    }
}

#[tokio::test]
async fn list_directory_registrations_on_an_empty_store_reports_an_empty_page() {
    let (router, _engine) = router();

    for max_results in [None, Some(1), Some(1000)] {
        let (status, _, parsed) = list_page(&router, max_results, None).await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results:?}: ListDirectoryRegistrations must answer 200, \
             got {status} / {parsed}"
        );
        assert_eq!(
            page_arns(&parsed),
            Vec::<String>::new(),
            "case MaxResults={max_results:?}: a store holding nothing lists nothing, got {parsed}"
        );
        assert!(
            parsed["NextToken"].is_null(),
            "case MaxResults={max_results:?}: an exhausted listing carries no NextToken, \
             got {parsed}"
        );
    }
}

#[tokio::test]
async fn list_directory_registrations_pages_through_every_registration_once_in_creation_order() {
    let (router, _engine) = router();

    let mut created = Vec::new();
    for directory_id in DIRECTORY_IDS {
        created.push(create_registration(&router, directory_id).await);
    }

    // Page sizes spanning "divides evenly", "leaves a remainder", "exactly the
    // collection" and "larger than the collection".
    for page_size in [1usize, 2, 4, DIRECTORY_IDS.len(), 1000] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, _, parsed) = list_page(&router, Some(page_size), token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case page_size={page_size}: ListDirectoryRegistrations must answer 200, \
                 got {status} / {parsed}"
            );
            let page = page_arns(&parsed);
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case page_size={page_size}: a page must hold between one and MaxResults \
                 entries, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case page_size={page_size}: paging did not terminate after {pages} pages"
            );
            match parsed["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, created,
            "case page_size={page_size}: paging must yield every registration exactly once \
             in creation order"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            created.len(),
            "case page_size={page_size}: paging must not duplicate a registration, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_listed_registration_carries_the_same_summary_the_read_path_reports() {
    let (router, engine) = router();

    let mut arns = Vec::new();
    for directory_id in &DIRECTORY_IDS[..3] {
        let arn = create_registration(&router, directory_id).await;
        set_settle_due(&engine, &arn, SystemTime::now() + NOT_DUE_FOR);
        arns.push(arn);
    }

    let (_, _, listed) = list_page(&router, None, None).await;
    let entries = listed["DirectoryRegistrations"]
        .as_array()
        .unwrap_or_else(|| panic!("the listing must carry DirectoryRegistrations, got {listed}"))
        .clone();
    assert_eq!(
        entries.len(),
        arns.len(),
        "the listing must report every stored registration, got {listed}"
    );

    for (arn, summary) in arns.iter().zip(entries.iter()) {
        let (_, _, read) = get_registration(&router, arn).await;
        let registration = &read["DirectoryRegistration"];
        for member in ["Arn", "DirectoryId", "Status", "CreatedAt", "UpdatedAt"] {
            assert_eq!(
                summary[member], registration[member],
                "case {arn} / {member}: the summary and the read must agree, \
                 got {summary} vs {registration}"
            );
        }
    }
}

#[tokio::test]
async fn a_registration_settles_out_of_creating_when_the_clock_owes_the_move() {
    let (router, engine) = router();
    let arn = create_registration(&router, DIRECTORY_IDS[0]).await;

    // Until the activation instant the registration is the CREATING the create's
    // 202 reported, and reading it repeatedly does not advance it.
    set_settle_due(&engine, &arn, SystemTime::now() + NOT_DUE_FOR);
    for attempt in 0..2 {
        let (_, _, parsed) = get_registration(&router, &arn).await;
        assert_eq!(
            parsed["DirectoryRegistration"]["Status"].as_str(),
            Some("CREATING"),
            "case attempt {attempt}: reading an unactivated registration must not advance it, \
             got {parsed}"
        );
    }
    let (_, _, held) = list_page(&router, None, None).await;
    assert_eq!(
        held["DirectoryRegistrations"][0]["Status"].as_str(),
        Some("CREATING"),
        "the listing must report the same unactivated status, got {held}"
    );

    // Once the instant has passed, both read paths report the move the clock owed.
    // This router mounts nothing that creates directories, so the account holds no
    // directory of that id: the authorization the registration asked for has
    // nothing to act on, and the terminal state is the failure with its reason
    // code. The ACTIVE branch, for a directory the account DOES hold, is in
    // `directory_registration_authorization`.
    set_settle_due(&engine, &arn, SystemTime::now() - ALREADY_DUE);
    let (_, _, settled) = get_registration(&router, &arn).await;
    assert_eq!(
        settled["DirectoryRegistration"]["Status"].as_str(),
        Some("FAILED"),
        "a registration of a directory the account does not hold terminates in FAILED, \
         got {settled}"
    );
    assert_eq!(
        settled["DirectoryRegistration"]["StatusReason"].as_str(),
        Some("DIRECTORY_RESOURCE_NOT_FOUND"),
        "the failure must name why the directory could not be authorized, got {settled}"
    );
    assert_eq!(
        settled["DirectoryRegistration"]["Arn"].as_str(),
        Some(arn.as_str()),
        "settling must not change the registration's identity, got {settled}"
    );
    let (_, _, listed) = list_page(&router, None, None).await;
    assert_eq!(
        listed["DirectoryRegistrations"][0]["Status"].as_str(),
        Some("FAILED"),
        "the listing must report the settled status too, got {listed}"
    );
    assert_eq!(
        listed["DirectoryRegistrations"][0]["StatusReason"].as_str(),
        Some("DIRECTORY_RESOURCE_NOT_FOUND"),
        "the listing must report the settled reason too, got {listed}"
    );
}

#[tokio::test]
async fn list_directory_registrations_refuses_a_next_token_it_never_issued() {
    let (router, _engine) = router();
    create_registration(&router, DIRECTORY_IDS[0]).await;

    // Every value here satisfies the modelled `NextToken` pattern (base64url
    // characters in groups of four), so the refusal comes from the handler's
    // cursor check and not from the boundary validator. The rows span the ways a
    // token can be foreign: no service tag at all, the tag alone, the tag with a
    // cursor field of the wrong width, the tag with a field that is not hex, a
    // wrong-case tag, and a bare cursor field.
    let foreign = [
        "YWJj",
        "pcad",
        "pcad0001",
        "pcadzzzzzzzzzzzzzzzz",
        "pcaD0000000000000001",
        "0000000000000001",
    ];
    for token in foreign {
        let (status, code, parsed) = list_page(&router, None, Some(token)).await;
        assert_eq!(
            status, 400,
            "case {token}: a token this service never issued is a 400, got {status} / {parsed}"
        );
        assert_eq!(
            code, "ValidationException",
            "case {token}: expected ValidationException, got {code:?} / {parsed}"
        );
        let message = parsed["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with("Pagination token exception in operation \
                                 'ListDirectoryRegistrations': "),
            "case {token}: the refusal must read as a pagination-token failure of this \
             operation, got {message:?}"
        );
    }
}

#[tokio::test]
async fn one_directory_carries_at_most_one_registration() {
    let (router, _engine) = router();

    for directory_id in &DIRECTORY_IDS[..3] {
        let arn = create_registration(&router, directory_id).await;

        // A second create for the same directory has no identity of its own to
        // answer with, so it is refused rather than stored.
        let (status, code, parsed) = call(
            &router,
            "POST",
            "/directoryRegistrations",
            Some(&json!({ "DirectoryId": directory_id })),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {directory_id}: registering one directory twice is a conflict, \
             got {status} / {parsed}"
        );
        assert_eq!(
            code, "ConflictException",
            "case {directory_id}: expected ConflictException, got {code:?} / {parsed}"
        );
        assert_eq!(
            parsed["ResourceId"].as_str(),
            Some(arn.as_str()),
            "case {directory_id}: the conflict must name the registration that holds the \
             directory, got {parsed}"
        );
        assert_eq!(
            parsed["ResourceType"].as_str(),
            Some("DirectoryRegistration"),
            "case {directory_id}: the conflict must name the resource type, got {parsed}"
        );

        // A ClientToken this service has not seen before is not a retry, so it
        // does not make a taken directory free either.
        let body = json!({
            "DirectoryId": directory_id,
            "ClientToken": format!("token-{directory_id}"),
        });
        let (fresh_status, _, fresh) =
            call(&router, "POST", "/directoryRegistrations", Some(&body)).await;
        assert_eq!(
            fresh_status, 409,
            "case {directory_id}: a ClientToken the service never saw is not a retry, \
             got {fresh_status} / {fresh}"
        );
    }

    // The refused creates stored nothing: the store holds exactly one registration
    // per directory addressed.
    let (_, _, listed) = list_page(&router, None, None).await;
    assert_eq!(
        page_arns(&listed).len(),
        3,
        "a refused create must not have been stored, got {listed}"
    );
}

#[tokio::test]
async fn a_repeated_client_token_returns_the_first_registration_rather_than_a_second_one() {
    let (router, _engine) = router();

    let body = json!({ "DirectoryId": DIRECTORY_IDS[0], "ClientToken": "token-registration-1" });
    let (first_status, _, first) =
        call(&router, "POST", "/directoryRegistrations", Some(&body)).await;
    assert_eq!(
        first_status, 202,
        "the first create must succeed, got {first_status} / {first}"
    );
    let (repeat_status, _, repeat) =
        call(&router, "POST", "/directoryRegistrations", Some(&body)).await;
    assert_eq!(
        repeat_status, 202,
        "a retry carrying the same ClientToken must succeed, got {repeat_status} / {repeat}"
    );
    assert_eq!(
        repeat["DirectoryRegistrationArn"], first["DirectoryRegistrationArn"],
        "the same ClientToken must name one registration, got {repeat} vs {first}"
    );

    let other = json!({ "DirectoryId": DIRECTORY_IDS[1], "ClientToken": "token-registration-2" });
    let (status, _, second) = call(&router, "POST", "/directoryRegistrations", Some(&other)).await;
    assert_eq!(
        status, 202,
        "another directory with another token mints another registration, got {status} / {second}"
    );
    assert_ne!(
        second["DirectoryRegistrationArn"], first["DirectoryRegistrationArn"],
        "another directory must not name the first registration, got {second} vs {first}"
    );

    // The token is scoped to the directory the request addressed, so reusing it
    // against a different directory registers THAT directory instead of answering
    // with an ARN the caller did not ask about.
    let reused = json!({ "DirectoryId": DIRECTORY_IDS[2], "ClientToken": "token-registration-1" });
    let (status, _, third) = call(&router, "POST", "/directoryRegistrations", Some(&reused)).await;
    assert_eq!(
        status, 202,
        "a token reused against another directory must register that directory, \
         got {status} / {third}"
    );
    assert_eq!(
        third["DirectoryRegistrationArn"].as_str(),
        Some(format!("{}{}", registration_arn_prefix(), DIRECTORY_IDS[2]).as_str()),
        "a token reused against another directory must name THAT directory, \
         got {third} vs {first}"
    );

    let (_, _, listed) = list_page(&router, None, None).await;
    assert_eq!(
        page_arns(&listed).len(),
        3,
        "the store must hold exactly the three registrations created, got {listed}"
    );
}

#[tokio::test]
async fn the_directory_registration_store_is_bounded_and_refuses_past_its_quota() {
    let (router, _engine) = router();

    // A distinct directory id per create, so nothing is refused as a duplicate on
    // the way to the bound.
    let directory_id = |index: usize| format!("d-{index:010x}");

    for index in 0..MAX_DIRECTORY_REGISTRATIONS {
        let (status, _, parsed) = call(
            &router,
            "POST",
            "/directoryRegistrations",
            Some(&json!({ "DirectoryId": directory_id(index) })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case registration {index}: every create up to the bound must succeed, \
             got {status} / {parsed}"
        );
    }

    let (status, code, parsed) = call(
        &router,
        "POST",
        "/directoryRegistrations",
        Some(&json!({ "DirectoryId": directory_id(MAX_DIRECTORY_REGISTRATIONS) })),
    )
    .await;
    assert_eq!(
        status, 402,
        "case one past the bound: the create must be refused, got {status} / {parsed}"
    );
    assert_eq!(
        code, "ServiceQuotaExceededException",
        "case one past the bound: expected ServiceQuotaExceededException, got {code:?} / {parsed}"
    );
    assert_eq!(
        parsed["ResourceType"].as_str(),
        Some("DirectoryRegistration"),
        "case one past the bound: the refusal must name the resource type, got {parsed}"
    );

    // The refusal did not grow the store: exactly the bound is held.
    let (_, _, listed) = list_page(&router, Some(MAX_DIRECTORY_REGISTRATIONS), None).await;
    assert_eq!(
        page_arns(&listed).len(),
        MAX_DIRECTORY_REGISTRATIONS,
        "a refused create must not have been stored, got {} registrations",
        page_arns(&listed).len()
    );
}
