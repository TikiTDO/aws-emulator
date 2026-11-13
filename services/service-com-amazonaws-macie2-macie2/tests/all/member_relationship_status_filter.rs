// SPDX-License-Identifier: CC0-1.0

//! Which accounts `ListMembers` reports, by the status of each account's
//! relationship with the administrator account.
//!
//! The rule under test: a request that does not ask for all accounts reports
//! only CURRENT member accounts — the accounts whose membership with this
//! administrator account is established and still live. An account this
//! administrator merely has a record of — an association no membership
//! invitation has been sent for, an invitation that never reached the account,
//! a membership that ended, or an account that cannot be administered here —
//! is reported only to a request that sets `onlyAssociated` to false. An
//! id-addressed read is not filtered at all: it reports the association
//! whatever its status.
//!
//! Every case is asserted over a SET of inputs: the whole modelled
//! `RelationshipStatus` catalog for the filter, the page sizes the filtered
//! enumeration is walked with, and the `onlyAssociated` spellings a caller
//! sends.

use std::collections::BTreeMap;
use std::time::SystemTime;

use service_com_amazonaws_macie2_macie2 as macie2;
use smithy_server_runtime::MultiServiceRouter;

use super::macie_session_and_custom_data_identifiers::{call_in, emulator_with_engine, enable_in};

/// The region every case signs for.
const REGION: &str = "us-west-2";

/// The member collection's URI, from `ListMembers`'s `@http` binding.
const MEMBERS_URI: &str = "/members";

/// One row of the status catalog: a `RelationshipStatus` and whether an account
/// carrying it is a current member of its administrator account, so a request
/// that does not ask for all accounts reports it.
struct StatusRow {
    status: &'static str,
    is_current_member: bool,
}

/// Every value of the modelled `RelationshipStatus` enum —
/// `smithy_query(query="shape",shape="com.amazonaws.macie2#RelationshipStatus")`
/// lists exactly these ten — against whether the account is a current member.
///
/// A membership is live under `Enabled` (in force), `Paused` (in force, with
/// Macie suspended for the account) and `Invited` (the invitation is
/// outstanding). Under the rest the administrator has a record of the account
/// but no live membership with it: nothing has been sent for the association
/// yet (`Created`), the invitation never reached the account
/// (`EmailVerificationInProgress`, `EmailVerificationFailed`), the membership
/// ended (`Removed`, `Resigned`), or the account cannot be administered here
/// (`RegionDisabled`, `AccountSuspended`).
const STATUS_CATALOG: [StatusRow; 10] = [
    StatusRow {
        status: "Enabled",
        is_current_member: true,
    },
    StatusRow {
        status: "Paused",
        is_current_member: true,
    },
    StatusRow {
        status: "Invited",
        is_current_member: true,
    },
    StatusRow {
        status: "Created",
        is_current_member: false,
    },
    StatusRow {
        status: "Removed",
        is_current_member: false,
    },
    StatusRow {
        status: "Resigned",
        is_current_member: false,
    },
    StatusRow {
        status: "EmailVerificationInProgress",
        is_current_member: false,
    },
    StatusRow {
        status: "EmailVerificationFailed",
        is_current_member: false,
    },
    StatusRow {
        status: "RegionDisabled",
        is_current_member: false,
    },
    StatusRow {
        status: "AccountSuspended",
        is_current_member: false,
    },
];

/// One emulator, with the shared engine kept so a case can seed an association
/// in a status no implemented operation reaches yet.
fn seed_association(
    engine: &engine::InMemoryEngine,
    region: &str,
    account: &str,
    status: &str,
) {
    let store = macie2::state::service_state(engine).expect("the Macie store is on this engine");
    let now = SystemTime::now();
    let mut state = store.lock();
    state.members.insert(
        (region.to_string(), account.to_string()),
        macie2::state::Member {
            arn: macie2::state::member_arn(region, account),
            account_id: account.to_string(),
            email: format!("{account}@example.com"),
            region: region.to_string(),
            administrator_account_id: ::engine::emulator_account_id().to_string(),
            relationship_status: status.to_string(),
            tags: BTreeMap::new(),
            // The association starts with no invitation instant; every other
            // status follows a call that sent one.
            invited_at: (status != "Created").then_some(now),
            updated_at: now,
        },
    );
}

/// Walk the whole enumeration and hand back the account ids it reports, in the
/// order it reports them. `only_associated` and `max_results` are sent only
/// when named, so the `None` row is the request a caller makes with neither.
async fn list_accounts(
    router: &MultiServiceRouter,
    case: &str,
    only_associated: Option<&str>,
    max_results: Option<u32>,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One request per possible page, plus one, so a token that never clears is
    // a failure rather than a hang.
    for _ in 0..=STATUS_CATALOG.len() {
        let mut query: Vec<String> = Vec::new();
        if let Some(value) = only_associated {
            query.push(format!("onlyAssociated={value}"));
        }
        if let Some(size) = max_results {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = token.as_deref() {
            query.push(format!("nextToken={token}"));
        }
        let uri = if query.is_empty() {
            MEMBERS_URI.to_string()
        } else {
            format!("{MEMBERS_URI}?{}", query.join("&"))
        };

        let (status, page) = call_in(router, REGION, "GET", &uri, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListMembers must answer 200, got {status} with {page:?}"
        );
        let items = page["members"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: a page must carry a members array, got {page:?}")
        });
        if let Some(size) = max_results {
            assert!(
                items.len() <= usize::try_from(size).unwrap_or_default(),
                "case {case:?}: a page must not exceed maxResults {size}, got {} items",
                items.len()
            );
        }
        for item in items {
            seen.push(
                item["accountId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: every member carries its accountId, got {item:?}")
                    })
                    .to_string(),
            );
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
    seen
}

/// The twelve-digit account id of the catalog row at `index`.
fn account_of(index: usize) -> String {
    format!("5555{index:08}")
}

/// The default view reports exactly the accounts whose membership is live, and
/// the view that asks for all accounts reports every association — asserted
/// over the whole modelled status catalog, over the page sizes the filtered
/// enumeration is walked with, and over the `onlyAssociated` spellings a caller
/// sends. An id-addressed read reports every association whatever its status.
#[tokio::test]
async fn the_default_view_reports_only_current_member_accounts() {
    let (router, engine) = emulator_with_engine();
    enable_in(&router, REGION, "{}").await;

    for (index, row) in STATUS_CATALOG.iter().enumerate() {
        seed_association(&engine, REGION, &account_of(index), row.status);
    }

    let mut all: Vec<String> = (0..STATUS_CATALOG.len()).map(account_of).collect();
    all.sort();
    let mut current: Vec<String> = STATUS_CATALOG
        .iter()
        .enumerate()
        .filter(|(_, row)| row.is_current_member)
        .map(|(index, _)| account_of(index))
        .collect();
    current.sort();

    // The spellings a caller sends for each view, and what each one reports.
    // `onlyAssociated` is a bare string, and only "false" asks for all
    // accounts (`ListMembersRequest$onlyAssociated`).
    let views: [(&str, Option<&str>, &Vec<String>); 5] = [
        ("onlyAssociated unset", None, &current),
        ("onlyAssociated=true", Some("true"), &current),
        ("onlyAssociated=TRUE", Some("TRUE"), &current),
        ("onlyAssociated=false", Some("false"), &all),
        ("onlyAssociated=False", Some("False"), &all),
    ];

    for (view, only_associated, expected) in views {
        for max_results in [None, Some(1), Some(3), Some(25)] {
            let case = format!("{view} with maxResults {max_results:?}");
            let mut seen = list_accounts(&router, &case, only_associated, max_results).await;
            seen.sort();
            assert_eq!(
                &seen, expected,
                "case {case:?}: expected {expected:?}, got {seen:?}"
            );
        }
    }

    // Every seeded association is readable by id, whatever its status: the
    // filter is the listing's, not the store's.
    for (index, row) in STATUS_CATALOG.iter().enumerate() {
        let account = account_of(index);
        let (status, record) = call_in(
            &router,
            REGION,
            "GET",
            &format!("{MEMBERS_URI}/{account}"),
            "",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the association must be readable by id, got {status} with {record:?}",
            row.status
        );
        assert_eq!(
            record["relationshipStatus"].as_str(),
            Some(row.status),
            "case {:?}: the read reports the stored status, got {record:?}",
            row.status
        );
    }
}

/// End to end over the wire, with no seeding: `CreateMember` associates an
/// account no membership invitation has been sent for, so the default view
/// leaves it out and only the view that asks for all accounts reports it. The
/// rows are the account ids the creates name.
#[tokio::test]
async fn an_association_no_invitation_has_been_sent_for_is_left_out_of_the_default_view() {
    let (router, _engine) = emulator_with_engine();
    enable_in(&router, REGION, "{}").await;

    let accounts = ["111122223333", "444455556666", "777788889999"];
    for account in accounts {
        let body = serde_json::json!({
            "account": { "accountId": account, "email": format!("{account}@example.com") }
        });
        let (status, reply) =
            call_in(&router, REGION, "POST", MEMBERS_URI, &body.to_string()).await;
        assert_eq!(
            status, 200,
            "case {account:?}: CreateMember must answer 200, got {status} with {reply:?}"
        );
    }

    let mut expected: Vec<String> = accounts.iter().map(|id| (*id).to_string()).collect();
    expected.sort();

    for (case, only_associated, want) in [
        ("onlyAssociated unset", None, Vec::new()),
        ("onlyAssociated=true", Some("true"), Vec::new()),
        ("onlyAssociated=false", Some("false"), expected.clone()),
    ] {
        let mut seen = list_accounts(&router, case, only_associated, None).await;
        seen.sort();
        assert_eq!(
            seen, want,
            "case {case:?}: expected {want:?}, got {seen:?}"
        );
    }
}
