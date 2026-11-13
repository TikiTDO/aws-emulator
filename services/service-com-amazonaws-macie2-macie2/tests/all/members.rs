// SPDX-License-Identifier: CC0-1.0

//! The Macie member plane — associating an account with this administrator
//! account and reading the association back — end to end over the wire.
//!
//! Each case drives the production `MultiServiceRouter` through the harness
//! `macie_session_and_custom_data_identifiers` builds, so what is pinned is the
//! HTTP behaviour and the response BODIES rather than a handler called
//! directly.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! request:
//!
//! * `CreateMember` refuses the account the request is made BY — an account
//!   cannot become its own member — with 400 `ValidationException`, and accepts
//!   every other account id, so the table below is the same call with the id
//!   varied;
//! * a refused create stores nothing, and an accepted one PERSISTS every
//!   member of the request, so `GetMember` reports the stored association:
//!   the account id and email as sent, the administrator account, the
//!   relationship status the association starts in, and no invitation instant
//!   until one is sent;
//! * `GetMember` refuses an account this administrator holds no association
//!   for, whatever the id looks like;
//! * an association belongs to the region the create was signed for, and both
//!   operations need a Macie session in that region;
//! * `ListMembers` asked for all accounts enumerates exactly the associations a
//!   region holds, paging by `maxResults` and resuming from its own
//!   `nextToken`, while its default view reports only current member accounts
//!   (which is `member_relationship_status_filter`'s rule);
//! * associating an account that is already a member states that one
//!   association again rather than doubling it.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use super::macie_session_and_custom_data_identifiers::{
    call_in, emulator, enable_in, error_code, error_message,
};

/// The region most cases sign for; it must reach every minted ARN.
const REGION: &str = "us-west-2";

/// A second region, for the cases that pin the per-region scope of an
/// association.
const OTHER_REGION: &str = "eu-west-1";

/// The member collection's URI, from `CreateMember`'s `@http` binding.
const MEMBERS_URI: &str = "/members";

/// The `RelationshipStatus` an association carries once it has been created and
/// before any membership invitation is sent. An entry of the modelled
/// `RelationshipStatus` enum (`smithy_query(query="shape",shape="RelationshipStatus")`).
const CREATED_RELATIONSHIP_STATUS: &str = "Created";

/// How the service refuses an attempt to associate the caller's own account.
/// The whole sentence is fixed text, so it is spelled out here rather than
/// built from the request.
const SELF_MEMBERSHIP_MESSAGE: &str =
    "The request is rejected because an account cannot become a member of itself.";

/// How the service refuses a read of an account it holds no association for.
const NOT_AN_ASSOCIATED_MEMBER_MESSAGE: &str =
    "The request is rejected because the given account ID is not an associated member of \
     the current account.";

/// The account every emulated request is signed for, which is the
/// administrator account of every association.
fn own_account() -> String {
    ::engine::emulator_account_id().to_string()
}

/// Associate `account` in `region`, with the request body a caller sends.
async fn create(
    router: &MultiServiceRouter,
    region: &str,
    account: &str,
    email: &str,
    tags: Option<&BTreeMap<String, String>>,
) -> (u16, serde_json::Value) {
    let mut body = serde_json::json!({
        "account": { "accountId": account, "email": email }
    });
    if let Some(tags) = tags {
        body["tags"] = serde_json::json!(tags);
    }
    call_in(router, region, "POST", MEMBERS_URI, &body.to_string()).await
}

/// Read the association of `account` in `region`.
async fn read(
    router: &MultiServiceRouter,
    region: &str,
    account: &str,
) -> (u16, serde_json::Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("{MEMBERS_URI}/{account}"),
        "",
    )
    .await
}

/// The ARN an association of `account` in `region` carries, built the way every
/// Macie ARN is: the service, the region the call addressed, the emulator
/// account and the resource.
fn expected_arn(region: &str, account: &str) -> String {
    format!(
        "arn:aws:macie2:{region}:{}:member/{account}",
        ::engine::emulator_account_id()
    )
}

/// The create is refused for exactly one account id — the caller's own — and
/// accepted for every other, and a refused create stores nothing: the read
/// that follows it finds no association.
#[tokio::test]
async fn a_create_refuses_the_callers_own_account_and_accepts_any_other() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let own = own_account();
    // The rows vary along the one thing the rule is stated over: the account id
    // the request names. Two of them differ from the caller's own in a single
    // digit, at each end, so what is asserted is the comparison and not a
    // resemblance.
    let bumped_first: String = own
        .chars()
        .enumerate()
        .map(|(index, digit)| if index == 0 { '9' } else { digit })
        .collect();
    let last_index = own.chars().count().saturating_sub(1);
    let bumped_last: String = own
        .chars()
        .enumerate()
        .map(|(index, digit)| if index == last_index { '7' } else { digit })
        .collect();
    let rows: Vec<(&str, String)> = vec![
        ("the caller's own account", own.clone()),
        ("another twelve-digit account", "111122223333".to_string()),
        ("a third twelve-digit account", "210987654321".to_string()),
        ("the caller's id with its first digit changed", bumped_first),
        ("the caller's id with its last digit changed", bumped_last),
    ];

    for (case, account) in rows {
        let must_refuse = account == own;
        let (status, reply) = create(
            &router,
            REGION,
            &account,
            &format!("{account}@example.com"),
            None,
        )
        .await;

        if must_refuse {
            assert_eq!(
                status, 400,
                "case {case:?}: an account cannot become a member of itself, \
                 got {status} with {reply:?}"
            );
            assert_eq!(
                error_code(&reply),
                "ValidationException",
                "case {case:?}: the refusal must be the modelled ValidationException, \
                 got {reply:?}"
            );
            assert_eq!(
                error_message(&reply),
                SELF_MEMBERSHIP_MESSAGE,
                "case {case:?}: expected {SELF_MEMBERSHIP_MESSAGE:?}, got {reply:?}"
            );

            // The refused call stored nothing, so there is no association to
            // read for that account.
            let (read_status, read_reply) = read(&router, REGION, &account).await;
            assert_eq!(
                read_status, 400,
                "case {case:?}: a refused create must leave no association, \
                 got {read_status} with {read_reply:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: an account other than the caller's is associated, \
                 got {status} with {reply:?}"
            );
            assert_eq!(
                reply["arn"].as_str(),
                Some(expected_arn(REGION, &account).as_str()),
                "case {case:?}: the ARN names the region and the account, got {reply:?}"
            );
        }
    }
}

/// One row of the create-and-read-back table: the case name, the account to
/// associate, the email the request carries for it, and the tags it names
/// (`None` when the request names no tags at all).
struct StoredRow {
    case: &'static str,
    account: &'static str,
    email: &'static str,
    tags: Option<BTreeMap<String, String>>,
}

/// An accepted create PERSISTS every member of the request, so the read reports
/// the stored association. The rows vary the email and the tag map, including
/// the forms where the request names no tags at all and where it names an empty
/// map — both of which leave the read reporting no tags.
#[tokio::test]
async fn a_created_association_is_read_back_exactly_as_stored() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let one_tag: BTreeMap<String, String> = [("team".to_string(), "security".to_string())]
        .into_iter()
        .collect();
    let several_tags: BTreeMap<String, String> = [
        ("cost-centre".to_string(), "4711".to_string()),
        ("owner".to_string(), "platform".to_string()),
        ("tier".to_string(), "gold".to_string()),
    ]
    .into_iter()
    .collect();

    let rows: Vec<StoredRow> = vec![
        StoredRow {
            case: "no tags member",
            account: "111122223333",
            email: "a@example.com",
            tags: None,
        },
        StoredRow {
            case: "an empty tag map",
            account: "222233334444",
            email: "b@example.net",
            tags: Some(BTreeMap::new()),
        },
        StoredRow {
            case: "one tag",
            account: "333344445555",
            email: "c+macie@example.org",
            tags: Some(one_tag),
        },
        StoredRow {
            case: "several tags",
            account: "444455556666",
            email: "d@subdomain.example.com",
            tags: Some(several_tags),
        },
    ];

    for StoredRow {
        case,
        account,
        email,
        tags,
    } in rows
    {
        let (status, reply) = create(&router, REGION, account, email, tags.as_ref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateMember must answer 200, got {status} with {reply:?}"
        );

        let (read_status, record) = read(&router, REGION, account).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: the association must be readable, got {read_status} with {record:?}"
        );
        assert_eq!(
            record["accountId"].as_str(),
            Some(account),
            "case {case:?}: the account id is echoed, got {record:?}"
        );
        assert_eq!(
            record["email"].as_str(),
            Some(email),
            "case {case:?}: the email is echoed, got {record:?}"
        );
        assert_eq!(
            record["arn"].as_str(),
            Some(expected_arn(REGION, account).as_str()),
            "case {case:?}: the stored ARN is reported, got {record:?}"
        );
        assert_eq!(
            record["administratorAccountId"].as_str(),
            Some(own_account().as_str()),
            "case {case:?}: the administrator is the account the create was made by, \
             got {record:?}"
        );
        assert_eq!(
            record["masterAccountId"], record["administratorAccountId"],
            "case {case:?}: the deprecated member carries the administrator account, \
             got {record:?}"
        );
        assert_eq!(
            record["relationshipStatus"].as_str(),
            Some(CREATED_RELATIONSHIP_STATUS),
            "case {case:?}: a created association starts in the status the service \
             assigns it, got {record:?}"
        );
        assert!(
            record["invitedAt"].is_null(),
            "case {case:?}: no membership invitation has been sent, so there is no \
             instant to report, got {record:?}"
        );
        assert!(
            record["updatedAt"]
                .as_str()
                .is_some_and(|instant| !instant.is_empty()),
            "case {case:?}: the instant the relationship last changed must be reported, \
             got {record:?}"
        );

        let reported: BTreeMap<String, String> = record["tags"]
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected = tags.unwrap_or_default();
        assert_eq!(
            reported, expected,
            "case {case:?}: the tags stored must be the tags reported, got {record:?}"
        );
    }
}

/// A read of an account this administrator holds no association for is refused
/// with the modelled `ValidationException` and the service's sentence, whatever
/// the id looks like. The rows span the id forms a caller sends: another
/// account, the caller's own (which can never be a member), a value that is not
/// account-shaped at all, and an account associated only in another region.
#[tokio::test]
async fn reading_an_account_that_is_not_an_associated_member_is_refused() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;
    enable_in(&router, OTHER_REGION, "{}").await;

    let elsewhere = "555566667777";
    let (status, reply) = create(
        &router,
        OTHER_REGION,
        elsewhere,
        "elsewhere@example.com",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"associate in another region\": expected 200, got {status} with {reply:?}"
    );

    let own = own_account();
    let rows: Vec<(&str, &str)> = vec![
        ("an account never associated", "111122223333"),
        ("the caller's own account", own.as_str()),
        ("a value that is not account-shaped", "not-an-account"),
        ("an account associated only in another region", elsewhere),
    ];

    for (case, id) in rows {
        let (status, reply) = read(&router, REGION, id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetMember must refuse an account that is not an associated \
             member, got {status} with {reply:?}"
        );
        assert_eq!(
            error_code(&reply),
            "ValidationException",
            "case {case:?}: the refusal must be the modelled ValidationException, \
             got {reply:?}"
        );
        assert_eq!(
            error_message(&reply),
            NOT_AN_ASSOCIATED_MEMBER_MESSAGE,
            "case {case:?}: expected {NOT_AN_ASSOCIATED_MEMBER_MESSAGE:?}, got {reply:?}"
        );
    }
}

/// An association belongs to the region the create was signed for: each region
/// reads only its own, and `DisableMacie` — which "deletes all settings and
/// resources for a Macie account" — takes that region's associations with it
/// and leaves another region's untouched.
#[tokio::test]
async fn an_association_belongs_to_the_region_it_was_made_in() {
    let router = emulator();
    let associations = [(REGION, "111122223333"), (OTHER_REGION, "222233334444")];

    for (region, account) in associations {
        enable_in(&router, region, "{}").await;
        let (status, reply) = create(
            &router,
            region,
            account,
            &format!("{account}@example.com"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: a create must answer 200, got {status} with {reply:?}"
        );
        assert_eq!(
            reply["arn"].as_str(),
            Some(expected_arn(region, account).as_str()),
            "case {region:?}: the ARN carries the region the create addressed, got {reply:?}"
        );
    }

    for (region, account) in associations {
        let (status, record) = read(&router, region, account).await;
        assert_eq!(
            status, 200,
            "case {region:?}: its own association must be readable, got {status} with {record:?}"
        );
        for (other, _) in associations.iter().filter(|(other, _)| *other != region) {
            let (status, reply) = read(&router, other, account).await;
            assert_eq!(
                status, 400,
                "case {region:?} read from {other:?}: an association of another region is \
                 not an associated member here, got {status} with {reply:?}"
            );
        }
    }

    let (status, reply) = call_in(&router, REGION, "DELETE", "/macie", "").await;
    assert_eq!(
        status, 200,
        "case \"disable Macie\": DisableMacie must answer 200, got {status} with {reply:?}"
    );
    enable_in(&router, REGION, "{}").await;

    let (status, reply) = read(&router, REGION, associations[0].1).await;
    assert_eq!(
        status, 400,
        "case \"after re-enabling Macie\": the association was deleted with the session, \
         got {status} with {reply:?}"
    );
    let (status, record) = read(&router, OTHER_REGION, associations[1].1).await;
    assert_eq!(
        status, 200,
        "case \"another region after a disable\": its association survives, \
         got {status} with {record:?}"
    );
}

/// Associating an account that is already a member states that one association
/// again: the ARN is the same, the details the second call names replace the
/// first call's, and there is still exactly one association to read. The rows
/// are the two members the second call can restate.
#[tokio::test]
async fn associating_the_same_account_again_restates_the_one_association() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let account = "111122223333";
    let (status, first) = create(&router, REGION, account, "first@example.com", None).await;
    assert_eq!(
        status, 200,
        "case \"first create\": expected 200, got {status} with {first:?}"
    );

    let tags: BTreeMap<String, String> = [("stage".to_string(), "second".to_string())]
        .into_iter()
        .collect();
    let (status, second) =
        create(&router, REGION, account, "second@example.com", Some(&tags)).await;
    assert_eq!(
        status, 200,
        "case \"restate the association\": expected 200, got {status} with {second:?}"
    );
    assert_eq!(
        second["arn"], first["arn"],
        "case \"restate the association\": the account id determines the ARN, so it is \
         unchanged, got {second:?} after {first:?}"
    );

    let (status, record) = read(&router, REGION, account).await;
    assert_eq!(
        status, 200,
        "case \"read the restated association\": expected 200, got {status} with {record:?}"
    );
    assert_eq!(
        record["email"].as_str(),
        Some("second@example.com"),
        "case \"read the restated association\": the details the second call named are the \
         stored ones, got {record:?}"
    );
    assert_eq!(
        record["tags"]["stage"].as_str(),
        Some("second"),
        "case \"read the restated association\": the tags the second call named are stored, \
         got {record:?}"
    );
}

/// `ListMembers` asked for all accounts enumerates exactly the associations the
/// region holds, paging by `maxResults` and resuming from its own `nextToken`,
/// and reports each one with the same values the id-addressed read does. The
/// rows are the page sizes the enumeration is walked with, so what is pinned is
/// the whole enumeration rather than one page.
///
/// Every association here was just created, so no membership invitation has
/// been sent for it and only the view that asks for all accounts reaches it.
#[tokio::test]
async fn the_listing_enumerates_exactly_the_associations_a_region_holds() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;
    enable_in(&router, OTHER_REGION, "{}").await;

    let accounts: Vec<String> = (1..=7)
        .map(|index| format!("1111222233{index:02}"))
        .collect();
    for account in &accounts {
        let (status, reply) = create(
            &router,
            REGION,
            account,
            &format!("{account}@example.com"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {account:?}: a create must answer 200, got {status} with {reply:?}"
        );
    }
    // One association in another region, which this region's enumeration must
    // not reach.
    let (status, reply) = create(
        &router,
        OTHER_REGION,
        "999988887777",
        "elsewhere@example.com",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"associate in another region\": expected 200, got {status} with {reply:?}"
    );

    let mut expected: Vec<String> = accounts.clone();
    expected.sort();

    for page_size in [None, Some(1), Some(3), Some(25)] {
        let case = format!("maxResults {page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One request per page, plus one, so a token that never clears is a
        // failure rather than a hang.
        for _ in 0..=expected.len() {
            let mut uri = MEMBERS_URI.to_string();
            // Asked for ALL accounts, which is what makes this the whole
            // enumeration of what the region holds.
            let mut query: Vec<String> = vec!["onlyAssociated=false".to_string()];
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = token.as_deref() {
                query.push(format!("nextToken={token}"));
            }
            if !query.is_empty() {
                uri.push('?');
                uri.push_str(&query.join("&"));
            }

            let (status, page) = call_in(&router, REGION, "GET", &uri, "").await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListMembers must answer 200, got {status} with {page:?}"
            );
            let items = page["members"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: a page must carry members, got {page:?}"));
            if let Some(size) = page_size {
                assert!(
                    items.len() <= usize::try_from(size).unwrap_or_default(),
                    "case {case:?}: a page must not exceed maxResults, got {page:?}"
                );
            }
            for item in items {
                let account = item["accountId"].as_str().unwrap_or_default().to_string();
                assert_eq!(
                    item["arn"].as_str(),
                    Some(expected_arn(REGION, &account).as_str()),
                    "case {case:?}: the summary carries the stored ARN, got {item:?}"
                );
                assert_eq!(
                    item["email"].as_str(),
                    Some(format!("{account}@example.com").as_str()),
                    "case {case:?}: the summary carries the stored email, got {item:?}"
                );
                assert_eq!(
                    item["relationshipStatus"].as_str(),
                    Some(CREATED_RELATIONSHIP_STATUS),
                    "case {case:?}: the summary carries the stored status, got {item:?}"
                );
                assert!(
                    item["updatedAt"]
                        .as_str()
                        .is_some_and(|instant| instant.ends_with('Z')),
                    "case {case:?}: an instant is reported as a UTC date-time string, \
                     got {item:?}"
                );
                seen.push(account);
            }

            token = page["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        assert!(
            token.is_none(),
            "case {case:?}: the enumeration must end, got a token after {seen:?}"
        );
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: the enumeration must report exactly what the region holds, \
             got {seen:?}"
        );
    }

    // `onlyAssociated` selects which accounts are reported: no association
    // here has a live membership yet, so only the view that asks for all
    // accounts reports them.
    for (only_associated, want) in [
        ("true", Vec::new()),
        ("false", expected.clone()),
        ("FALSE", expected.clone()),
    ] {
        let (status, page) = call_in(
            &router,
            REGION,
            "GET",
            &format!("{MEMBERS_URI}?onlyAssociated={only_associated}"),
            "",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {only_associated:?}: ListMembers must answer 200, got {status} with {page:?}"
        );
        let mut seen: Vec<String> = page["members"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["accountId"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        seen.sort();
        assert_eq!(
            seen, want,
            "case {only_associated:?}: expected {want:?}, got {seen:?}"
        );
    }

    // The other region enumerates only its own.
    let (status, page) = call_in(
        &router,
        OTHER_REGION,
        "GET",
        &format!("{MEMBERS_URI}?onlyAssociated=false"),
        "",
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"another region\": ListMembers must answer 200, got {status} with {page:?}"
    );
    let elsewhere: Vec<String> = page["members"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["accountId"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        elsewhere,
        vec!["999988887777".to_string()],
        "case \"another region\": each region enumerates its own associations, \
         got {elsewhere:?}"
    );
}

/// A `nextToken` this service did not issue names no page, so it is refused 400
/// `ValidationException` rather than read as "start again". The rows are the
/// token forms a client can send.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    for case in ["not-a-token", "abc", "ff", "%%%"] {
        let (status, reply) = call_in(
            &router,
            REGION,
            "GET",
            &format!("{MEMBERS_URI}?nextToken={case}"),
            "",
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign nextToken must answer 400, got {status} with {reply:?}"
        );
        assert_eq!(
            error_code(&reply),
            "ValidationException",
            "case {case:?}: the refusal must be the modelled ValidationException, \
             got {reply:?}"
        );
    }
}

/// Every operation of the member plane is refused 403 `AccessDeniedException` in
/// a region where nothing has enabled Macie: a member plane exists only under a
/// Macie session. The rows are the operations.
#[tokio::test]
async fn the_member_plane_needs_a_macie_session() {
    let router = emulator();

    let calls = [
        (
            "CreateMember",
            "POST",
            MEMBERS_URI.to_string(),
            serde_json::json!({
                "account": { "accountId": "111122223333", "email": "a@example.com" }
            })
            .to_string(),
        ),
        (
            "GetMember",
            "GET",
            format!("{MEMBERS_URI}/111122223333"),
            String::new(),
        ),
        ("ListMembers", "GET", MEMBERS_URI.to_string(), String::new()),
        (
            "CreateMember naming the caller's own account",
            "POST",
            MEMBERS_URI.to_string(),
            serde_json::json!({
                "account": { "accountId": own_account(), "email": "self@example.com" }
            })
            .to_string(),
        ),
    ];

    for (case, method, uri, body) in &calls {
        let (status, reply) = call_in(&router, REGION, method, uri, body).await;
        assert_eq!(
            status, 403,
            "case {case:?}: Macie is not enabled here, so the call is refused, \
             got {status} with {reply:?}"
        );
        assert_eq!(
            error_code(&reply),
            "AccessDeniedException",
            "case {case:?}: the refusal must be the modelled AccessDeniedException, \
             got {reply:?}"
        );
    }
}
