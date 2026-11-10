// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule `DescribeReportDefinitions` pagination rests on: a
//! continuation token is honoured only when THIS service issued it, and every other value
//! — however well formed, and whatever it looks like it names — is refused with a 400
//! `ValidationException`.
//!
//! The token is opaque: the position it names travels inside it behind an authenticator
//! computed under a secret the store holds. So the cases that matter are the ones a caller
//! could WRITE rather than round-trip: the hex of a name it saw on an earlier page, the
//! hex of an ordinal, a bare ordinal, a value with exactly the shape of a minted token,
//! one minted by another store, and a minted token with a byte changed. Each row is one
//! such value; the positive rows are the tokens the service itself handed back.

use serde_json::{json, Value};

use crate::report_definition_lifecycle::{error_code, stored, Cur};

/// The status and error code a page request carrying a token this service did not issue is
/// answered with. `ValidationException` is the refusal channel this operation's own model
/// leaves it (it declares no client-input error), and 400 is that error's modelled status.
const REFUSED: (u16, &str) = (400, "ValidationException");

/// The width of the authenticator a minted token carries, from the shared page-token
/// module, so a forged value can be given a minted token's exact shape without this suite
/// restating the width.
const AUTHENTICATOR_BYTES: usize = ::service_common::page_tokens::AUTHENTICATOR_BYTES;

/// `MaxResults` is pinned to 5 by the model (`smithy_query(shape="MaxResults")`: range
/// 5..5), so 5 is the page size every walk here is paged by.
const PAGE_SIZE: usize = 5;

/// The names of `count` seeded reports, in creation order.
fn seeded_names(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("paged-report-{index}"))
        .collect()
}

/// An emulator holding `count` report definitions. Seeded directly, because the
/// five-report cap is `PutReportDefinition`'s rule and pagination is only interesting
/// above it.
fn emulator_holding(count: usize) -> Cur {
    let cur = Cur::new();
    cur.seed(
        seeded_names(count)
            .iter()
            .map(|name| stored(name.as_str()))
            .collect(),
    );
    cur
}

/// The `NextToken` the first page of `cur` hands back.
async fn first_page_token(cur: &Cur) -> String {
    let (status, body) = cur.describe(&json!({})).await;
    assert_eq!(status, 200, "case first page: got {body:?}");
    body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case first page: expected a NextToken, got {body:?}"))
        .to_string()
}

/// A value with the shape of a minted token — `authenticator` bytes then the cursor — for
/// the position `resume_at`, rendered the way this service renders a token.
fn forged_token(authenticator: u8, resume_at: usize) -> String {
    let mut bytes = vec![authenticator; AUTHENTICATOR_BYTES];
    bytes.extend_from_slice(resume_at.to_string().as_bytes());
    hex::encode(bytes)
}

#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_refused() {
    // 11 reports: more than two pages, so every ordinal a caller might name is a position
    // the store really holds — the refusal is about ISSUANCE, not about whether the
    // position exists.
    let stored_count = 11_usize;
    let cur = emulator_holding(stored_count);
    let names = seeded_names(stored_count);

    // Another store holding the same reports: its tokens are minted under its own secret,
    // so they name no position here. This is the restarted-emulator case too.
    let elsewhere = emulator_holding(stored_count);
    let another_stores_token = first_page_token(&elsewhere).await;

    let mut cases: Vec<(String, String)> = vec![
        // The issue's instance: the hex of a report name the caller saw on a page.
        (
            "the hex of the first report's name".to_string(),
            hex::encode("paged-report-0"),
        ),
        (
            "another store's token for the same position".to_string(),
            another_stores_token,
        ),
        ("a bare ordinal".to_string(), "0".to_string()),
        ("a bare ordinal mid-store".to_string(), "5".to_string()),
        ("not hex at all".to_string(), "zzz".to_string()),
        ("an odd number of hex digits".to_string(), "6669727".to_string()),
        ("hex that is too short to carry an authenticator".to_string(), "ab".to_string()),
        ("empty-but-not-absent after an authenticator's width".to_string(), hex::encode(vec![0_u8; AUTHENTICATOR_BYTES])),
    ];
    // The hex of every name the store holds, and the hex of every ordinal into it: the two
    // families a caller can compute from what a page showed it.
    for (index, name) in names.iter().enumerate() {
        cases.push((format!("the hex of the name at {index}"), hex::encode(name)));
        cases.push((
            format!("the hex of the ordinal {index}"),
            hex::encode(index.to_string()),
        ));
        // A minted token's exact shape, with an authenticator the caller chose.
        cases.push((
            format!("a zero authenticator over position {index}"),
            forged_token(0x00, index),
        ));
        cases.push((
            format!("an all-ones authenticator over position {index}"),
            forged_token(0xff, index),
        ));
    }
    // A token this store DID mint, with one byte changed, one dropped and one appended.
    let issued = first_page_token(&cur).await;
    let issued_bytes = hex::decode(&issued).expect("an issued token is hex");
    for index in [0_usize, AUTHENTICATOR_BYTES - 1, AUTHENTICATOR_BYTES] {
        let mut tampered = issued_bytes.clone();
        tampered[index] ^= 0x01;
        cases.push((
            format!("an issued token with byte {index} changed"),
            hex::encode(tampered),
        ));
    }
    cases.push((
        "an issued token with its last byte dropped".to_string(),
        hex::encode(&issued_bytes[..issued_bytes.len() - 1]),
    ));
    cases.push((
        "an issued token with a byte appended".to_string(),
        hex::encode([issued_bytes.as_slice(), b"0"].concat()),
    ));

    for (case, token) in cases {
        let (status, body) = cur.describe(&json!({ "NextToken": token })).await;
        assert_eq!(
            (status, error_code(&body)),
            REFUSED,
            "case {case:?}: token {token:?} expected {REFUSED:?}, got {status} {body:?}"
        );
        assert!(
            body["ReportDefinitions"].is_null(),
            "case {case:?}: a refused page request reports no definitions, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_issued_next_token_is_opaque_and_resumes_the_walk_it_came_from() {
    // Row per store size: below the page size (no token at all), exactly one page, and
    // several sizes above it, so a walk spans one, two and three pages.
    for stored_count in [0_usize, 1, 5, 6, 11, 12] {
        let case = format!("{stored_count} stored");
        let cur = emulator_holding(stored_count);
        let names = seeded_names(stored_count);

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page_index in 0..stored_count + 2 {
            let mut request = json!({});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = cur.describe(&request).await;
            assert_eq!(status, 200, "case {case:?} page {page_index}: got {body:?}");
            let page = page_names(&body, &case, page_index);

            // Re-presenting the same token answers the same page: a read does not consume
            // the token it was handed, and nothing about the store changed.
            if let Some(token) = &token {
                let (status, again) = cur.describe(&json!({ "NextToken": token })).await;
                assert_eq!(
                    (status, page_names(&again, &case, page_index)),
                    (200, page.clone()),
                    "case {case:?} page {page_index}: the same token must answer the same page, \
                     got {again:?}"
                );
            }

            seen.extend(page);
            token = body["NextToken"].as_str().map(str::to_string);
            let Some(issued) = &token else { break };

            // The token is opaque: hex, inside the modelled NextToken length of 0..256,
            // and not any value the caller could have computed itself — not the ordinal
            // the page ended on, not its hex, and not the hex of any report name.
            let resumes_at = (page_index + 1) * PAGE_SIZE;
            assert!(
                issued.len() <= 256 && issued.chars().all(|char| char.is_ascii_hexdigit()),
                "case {case:?} page {page_index}: expected hex within 256 characters, \
                 got {issued:?}"
            );
            assert_ne!(
                *issued,
                resumes_at.to_string(),
                "case {case:?} page {page_index}: a token is not the ordinal it resumes at"
            );
            assert_ne!(
                *issued,
                hex::encode(resumes_at.to_string()),
                "case {case:?} page {page_index}: a token is not the hex of its ordinal"
            );
            for name in &names {
                assert_ne!(
                    *issued,
                    hex::encode(name),
                    "case {case:?} page {page_index}: a token is not the hex of a report name"
                );
            }
        }

        assert_eq!(
            token, None,
            "case {case:?}: the walk must end with no token"
        );
        assert_eq!(
            seen, names,
            "case {case:?}: expected every stored report exactly once in creation order, \
             got {seen:?}"
        );
    }
}

#[tokio::test]
async fn an_issued_next_token_is_refused_once_the_store_cannot_hold_its_position() {
    // The position a token names lives inside it, so a token stays valid while the store
    // still reaches that far and is refused once it does not: rows differ in how much of
    // the store is deleted under the token.
    let stored_count = 11_usize;
    for (case, deleted, accepted) in [
        ("nothing deleted", 0_usize, true),
        ("one report deleted", 1, true),
        ("down to the position the token names", stored_count - PAGE_SIZE, true),
        ("one past the position the token names", stored_count - PAGE_SIZE + 1, false),
        ("every report deleted", stored_count, false),
    ] {
        let cur = emulator_holding(stored_count);
        let token = first_page_token(&cur).await;
        for name in seeded_names(stored_count).iter().rev().take(deleted) {
            let (status, body) = cur
                .call("DeleteReportDefinition", &json!({ "ReportName": name }))
                .await;
            assert_eq!(status, 200, "case {case:?}: delete got {body:?}");
        }

        let (status, body) = cur.describe(&json!({ "NextToken": &token })).await;
        if accepted {
            assert_eq!(status, 200, "case {case:?}: got {body:?}");
            let expected: Vec<String> = seeded_names(stored_count - deleted)
                .into_iter()
                .skip(PAGE_SIZE)
                .take(PAGE_SIZE)
                .collect();
            assert_eq!(
                page_names(&body, case, 1),
                expected,
                "case {case:?}: expected the reports after the first page, got {body:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                REFUSED,
                "case {case:?}: expected {REFUSED:?}, got {status} {body:?}"
            );
        }
    }
}

/// The report names on one page of a `DescribeReportDefinitions` response.
fn page_names(body: &Value, case: &str, page_index: usize) -> Vec<String> {
    body["ReportDefinitions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?} page {page_index}: expected a list, got {body:?}"))
        .iter()
        .map(|definition| {
            definition["ReportName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?} page {page_index}: expected a name, got {definition:?}")
                })
                .to_string()
        })
        .collect()
}
