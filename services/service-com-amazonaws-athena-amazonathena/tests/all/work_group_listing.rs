// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListWorkGroups`: what the account's workgroup listing reports,
//! driven over the wire (awsJson1_1, `x-amz-target: AmazonAthena.<Operation>`) through
//! the same router the emulator boots, so every assertion is about what an SDK client
//! observes.
//!
//! The rule under test: a listing reports every workgroup the account holds — the
//! account's own default workgroup, plus each one a `CreateWorkGroup` call registered,
//! with the values those writes (and any later `UpdateWorkGroup`) persisted — in name
//! order, in pages of at most `MaxResults` entries, each page but the last followed by a
//! `NextToken` this service minted and no other value.
//!
//! Each rule is asserted over a TABLE rather than one request: the rows differ along what
//! the rule generalises over — how many workgroups the account holds, where each name
//! sorts, which optional members a create carried, the state an update left, the page
//! size asked for, and the form of a continuation token a caller sends back.

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    DEFAULT_EFFECTIVE_ENGINE_VERSION, DEFAULT_WORK_GROUP, ENGINE_VERSION_AUTO,
    ERROR_CODE_INVALID_INPUT, INVALID_NEXT_TOKEN, WORK_GROUP_STATE_DISABLED,
    WORK_GROUP_STATE_ENABLED,
};
use service_common::page_tokens::AUTHENTICATOR_BYTES;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_work_group, error_code, message, router};

/// The summaries a `ListWorkGroups` answer carries.
fn summaries(listed: &Value) -> Vec<Value> {
    listed["WorkGroups"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a listing must carry WorkGroups, got {listed:?}"))
}

/// The names those summaries carry, in the order the listing reported them.
fn names(listed: &Value) -> Vec<String> {
    summaries(listed)
        .iter()
        .map(|summary| {
            summary["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("every summary names its workgroup, got {summary:?}"))
                .to_string()
        })
        .collect()
}

/// The `NextToken` a page handed back, if it handed one back.
fn next_token(listed: &Value) -> Option<String> {
    listed["NextToken"].as_str().map(str::to_string)
}

/// One `ListWorkGroups` call that must be accepted.
async fn list(router: &MultiServiceRouter, request: &Value) -> Value {
    let (status, listed) = call(router, "ListWorkGroups", request).await;
    assert_eq!(
        status, 200,
        "ListWorkGroups({request}) must answer 200, got {status} with {listed:?}"
    );
    listed
}

/// An account nobody has written to holds exactly one workgroup: Athena's own default
/// workgroup, which no create call registered.
#[tokio::test]
async fn a_fresh_account_lists_its_default_work_group() {
    let router = router();
    let listed = list(&router, &json!({})).await;

    assert_eq!(
        names(&listed),
        vec![DEFAULT_WORK_GROUP.to_string()],
        "a fresh account holds the default workgroup alone, got {listed:?}"
    );
    let group = summaries(&listed).remove(0);
    assert_eq!(
        group["State"].as_str(),
        Some(WORK_GROUP_STATE_ENABLED),
        "the default workgroup is the one queries run in, so it is \
         {WORK_GROUP_STATE_ENABLED}, got {group:?}"
    );
    assert_eq!(
        group["EngineVersion"]["SelectedEngineVersion"].as_str(),
        Some(ENGINE_VERSION_AUTO),
        "the default workgroup selects no engine version of its own, got {group:?}"
    );
    assert_eq!(
        group["EngineVersion"]["EffectiveEngineVersion"].as_str(),
        Some(DEFAULT_EFFECTIVE_ENGINE_VERSION),
        "the default workgroup runs on the version Athena resolves for {ENGINE_VERSION_AUTO}, \
         got {group:?}"
    );
    assert!(
        group["CreationTime"].as_f64().is_some(),
        "the default workgroup reports when it came into being, got {group:?}"
    );
    assert_eq!(
        next_token(&listed),
        None,
        "a listing that reported everything hands back no token, got {listed:?}"
    );

    // A listing REPORTS: a second one sees exactly what the first one did, creation
    // instant included.
    let again = list(&router, &json!({})).await;
    assert_eq!(
        again, listed,
        "two listings of the same account must report the same workgroups, got {again:?} \
         after {listed:?}"
    );
}

/// One row of the registered-workgroup table.
struct Row {
    /// What this row varies.
    case: &'static str,
    /// The name the create call carried.
    name: String,
    /// The description it carried, if any.
    description: Option<&'static str>,
    /// The `SelectedEngineVersion` it carried, if any.
    selected_engine_version: Option<&'static str>,
    /// Whether a later `UpdateWorkGroup` disabled the workgroup.
    disabled: bool,
}

/// The rows of that table: names that sort before, around and after the default
/// workgroup's own name, with and without a description, with and without an engine
/// version of their own, and one left disabled by a later update.
///
/// `WorkGroupName` is `^[a-zA-Z0-9._-]{1,128}$`, so the longest name a client can send is
/// a row too.
fn registered_rows() -> Vec<Row> {
    vec![
        Row {
            case: "no description, a name sorting before the default workgroup",
            name: "alpha.analytics".to_string(),
            description: None,
            selected_engine_version: None,
            disabled: false,
        },
        Row {
            case: "a description, a name sorting after the default workgroup",
            name: "zeta_reporting".to_string(),
            description: Some("where the reporting queries run"),
            selected_engine_version: None,
            disabled: false,
        },
        Row {
            case: "an upper-case name, which sorts ahead of every lower-case one",
            name: "Engine-Pinned".to_string(),
            description: Some("pinned to one engine version"),
            // `ListEngineVersions` publishes the versions a workgroup may select; a
            // selected version is reported as the effective one unchanged.
            selected_engine_version: Some("Athena engine version 2"),
            disabled: false,
        },
        Row {
            case: "a name sharing its first character with the default workgroup's",
            name: "paused-group".to_string(),
            description: None,
            selected_engine_version: None,
            disabled: true,
        },
        Row {
            case: "a name of the longest modelled form",
            name: "w".repeat(128),
            description: None,
            selected_engine_version: None,
            disabled: false,
        },
    ]
}

/// Write each row's workgroup into `router`'s account, applying the row's update.
async fn register(router: &MultiServiceRouter, rows: &[Row]) {
    for row in rows {
        let mut request = json!({ "Name": row.name });
        if let Some(description) = row.description {
            request["Description"] = json!(description);
        }
        if let Some(version) = row.selected_engine_version {
            request["Configuration"] =
                json!({ "EngineVersion": { "SelectedEngineVersion": version } });
        }
        let (status, created) = call(router, "CreateWorkGroup", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateWorkGroup({request}) must answer 200, got {status} with {created:?}",
            row.case
        );

        if row.disabled {
            let update = json!({ "WorkGroup": row.name, "State": WORK_GROUP_STATE_DISABLED });
            let (status, updated) = call(router, "UpdateWorkGroup", &update).await;
            assert_eq!(
                status, 200,
                "case {:?}: UpdateWorkGroup({update}) must answer 200, got {status} with {updated:?}",
                row.case
            );
        }
    }
}

/// Every workgroup a write registered is reported, with the values that write persisted,
/// beside the account's default workgroup and in name order.
#[tokio::test]
async fn every_registered_work_group_is_listed_with_what_the_writes_persisted() {
    let router = router();
    let rows = registered_rows();
    register(&router, &rows).await;

    let listed = list(&router, &json!({ "MaxResults": 50 })).await;
    let reported = names(&listed);

    let mut expected: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    expected.push(DEFAULT_WORK_GROUP.to_string());
    expected.sort();
    assert_eq!(
        reported, expected,
        "expected every workgroup the account holds, in name order: {expected:?}, got \
         {reported:?}"
    );

    for row in &rows {
        let group = summaries(&listed)
            .into_iter()
            .find(|summary| summary["Name"].as_str() == Some(row.name.as_str()))
            .unwrap_or_else(|| {
                panic!("case {:?}: {} must be listed, got {listed:?}", row.case, row.name)
            });

        let expected_state = if row.disabled {
            WORK_GROUP_STATE_DISABLED
        } else {
            WORK_GROUP_STATE_ENABLED
        };
        assert_eq!(
            group["State"].as_str(),
            Some(expected_state),
            "case {:?}: expected the state {expected_state}, got {group:?}",
            row.case
        );
        assert_eq!(
            group["Description"].as_str(),
            row.description,
            "case {:?}: expected the description {:?}, got {group:?}",
            row.case,
            row.description
        );
        let expected_selected = row.selected_engine_version.unwrap_or(ENGINE_VERSION_AUTO);
        let expected_effective = row
            .selected_engine_version
            .unwrap_or(DEFAULT_EFFECTIVE_ENGINE_VERSION);
        assert_eq!(
            group["EngineVersion"]["SelectedEngineVersion"].as_str(),
            Some(expected_selected),
            "case {:?}: expected the selected engine version {expected_selected:?}, got \
             {group:?}",
            row.case
        );
        assert_eq!(
            group["EngineVersion"]["EffectiveEngineVersion"].as_str(),
            Some(expected_effective),
            "case {:?}: expected the effective engine version {expected_effective:?}, got \
             {group:?}",
            row.case
        );
        assert!(
            group["CreationTime"].as_f64().is_some(),
            "case {:?}: expected the summary to report when the workgroup was created, got \
             {group:?}",
            row.case
        );
    }
}

/// A paged walk reports every workgroup exactly once, in pages of at most the size asked
/// for, and hands back a token exactly while workgroups remain behind the page.
#[tokio::test]
async fn a_paged_listing_walks_every_work_group_exactly_once() {
    let router = router();
    // Seven registered workgroups beside the default one, so a page size divides the
    // total evenly in some rows and leaves a remainder in others.
    let registered: Vec<String> = (0..7).map(|index| format!("paged-{index}")).collect();
    for name in &registered {
        create_work_group(&router, name).await;
    }
    let mut expected: Vec<String> = registered.clone();
    expected.push(DEFAULT_WORK_GROUP.to_string());
    expected.sort();
    let total = expected.len();

    // Each row is a page size: one at a time, sizes that divide the total and sizes that
    // leave a remainder, the total itself, one past it, and the modelled maximum. The
    // last row asks for no size at all, which is the page a request naming none gets.
    for case in [Some(1), Some(2), Some(3), Some(5), Some(8), Some(9), Some(50), None] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({});
            if let Some(size) = case {
                request["MaxResults"] = json!(size);
            }
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let listed = list(&router, &request).await;
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case:?}: a token walk must terminate, walked {walked:?}"
            );

            let page = names(&listed);
            let page_size = case.unwrap_or(50).min(50) as usize;
            let remaining = total - walked.len();
            assert_eq!(
                page.len(),
                page_size.min(remaining),
                "case {case:?}: expected a page of {} after {walked:?}, got {page:?}",
                page_size.min(remaining)
            );
            walked.extend(page);

            token = next_token(&listed);
            let more = walked.len() < total;
            assert_eq!(
                token.is_some(),
                more,
                "case {case:?}: expected a NextToken to be present={more} after {walked:?}, \
                 got {listed:?}"
            );
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: expected the walk to report every workgroup once, in name order: \
             {expected:?}, got {walked:?}"
        );
        assert_eq!(
            pages,
            total.div_ceil(case.unwrap_or(50).min(50) as usize),
            "case {case:?}: expected the walk to take {} pages, took {pages}",
            total.div_ceil(case.unwrap_or(50).min(50) as usize)
        );
    }
}

/// A `NextToken` is "a token generated by the Athena service": a value this service did
/// not mint for THIS listing names no page, and is refused rather than answered with one.
#[tokio::test]
async fn a_next_token_this_service_did_not_mint_names_no_page() {
    let account = router();
    for index in 0..3 {
        create_work_group(&account, &format!("token-case-{index}")).await;
    }

    // A token another store minted for the same listing: another account's registry, so
    // another secret.
    let other_account = router();
    for index in 0..3 {
        create_work_group(&other_account, &format!("token-case-{index}")).await;
    }
    let other_store_token = next_token(&list(&other_account, &json!({ "MaxResults": 1 })).await)
        .expect("a page of one of four workgroups hands back a token");

    // A token THIS store minted for a DIFFERENT listing: a query history, which resumes
    // at a position in another sequence entirely.
    let execution_token = {
        let query = json!({
            "QueryString": "SELECT 1 AS one",
            "ResultConfiguration": { "OutputLocation": "s3://athena-results/listing-suite/" },
        });
        for _ in 0..2 {
            crate::harness::start(&account, &query).await;
        }
        let (status, listed) = call(
            &account,
            "ListQueryExecutions",
            &json!({ "WorkGroup": DEFAULT_WORK_GROUP, "MaxResults": 1 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "the query history must be readable, got {status} with {listed:?}"
        );
        next_token(&listed).expect("a page of one of two executions hands back a token")
    };

    // (case, the token sent)
    for (case, token) in [
        ("a value that is not a token at all", "made-up-token".to_string()),
        ("an empty-looking hex value", "00".to_string()),
        (
            "the shape of a minted token with an authenticator nobody computed",
            ::hex::encode(vec![0_u8; AUTHENTICATOR_BYTES + 16]),
        ),
        ("a token another store minted for this listing", other_store_token),
        ("a token this store minted for another listing", execution_token),
    ] {
        let (status, refused) = call(
            &account,
            "ListWorkGroups",
            &json!({ "MaxResults": 2, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            INVALID_NEXT_TOKEN,
            "case {case:?}: expected the sentence {INVALID_NEXT_TOKEN:?}, got {refused:?}"
        );
        assert_eq!(
            refused["AthenaErrorCode"].as_str(),
            Some(ERROR_CODE_INVALID_INPUT),
            "case {case:?}: expected AthenaErrorCode {ERROR_CODE_INVALID_INPUT:?}, got \
             {refused:?}"
        );
    }

    // The refusals left the listing itself alone.
    let listed = list(&account, &json!({ "MaxResults": 50 })).await;
    assert_eq!(
        names(&listed).len(),
        4,
        "expected the three registered workgroups and the default one, got {listed:?}"
    );
}
