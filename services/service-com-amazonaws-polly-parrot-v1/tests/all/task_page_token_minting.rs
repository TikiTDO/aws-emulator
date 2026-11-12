// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AUTHENTICITY of a `ListSpeechSynthesisTasks` `NextToken`,
//! driven over HTTP through the same router the emulator serves with.
//!
//! The rule here is about tokens the service did NOT issue, so every case is built to
//! have exactly the SHAPE of a minted token and to differ only in what a caller cannot
//! produce. The table spans the class: positions across the whole range of creation
//! instants a cursor can name (the epoch, the recent past, now, the far future, a
//! negative instant, the extremes of the cursor's integer), the filter the cursor
//! claims, authenticators a caller might try, a minted token with a byte changed at
//! every offset, a minted token truncated and extended, and a token minted by another
//! emulator.
//!
//! The harness and the readers come from `crate::lexicons_and_speech`, the listing
//! helpers from `crate::synthesis_tasks` and the task fixtures from `crate::support`.

use std::time::{Duration, SystemTime};

use crate::lexicons_and_speech::{error_code, json, Polly};
use crate::support::{fixture_task_id, seed_task, stored_task};
use crate::synthesis_tasks::{list, list_ids, task_ids};

/// The field separator a cursor uses, as `ListSpeechSynthesisTasks` mints it: the ASCII
/// unit separator, which no `TaskId` or `TaskStatus` value can contain.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// How many bytes of authenticator a minted token carries ahead of its cursor, taken
/// from the minter itself so a row built here keeps the SHAPE of a minted token however
/// wide the authenticator is.
const TAG_BYTES: usize = service_common::page_tokens::AUTHENTICATOR_BYTES;

/// A token with the SHAPE of a minted one: `tag` bytes, then the three-field cursor
/// `created_millis`, `task_id`, `status`, hex-encoded — which is what the service's own
/// `NextToken` is, except that no key authenticated it.
fn forged_token(tag: &[u8], created_millis: i64, task_id: &str, status: &str) -> String {
    let cursor = format!("{created_millis}{CURSOR_SEPARATOR}{task_id}{CURSOR_SEPARATOR}{status}");
    ::hex::encode([tag, cursor.as_bytes()].concat())
}

/// Seed `count` tasks, oldest first, half of them `completed` and half `failed`, and
/// answer their ids in the order the report must use — the reverse, because the report is
/// newest first.
fn seed_tasks(polly: &Polly, count: u64) -> Vec<String> {
    let now = SystemTime::now();
    let mut seeded: Vec<String> = (0..count)
        .map(|index| {
            let task_id = fixture_task_id(&format!("page-token-{index}"));
            let mut task = stored_task(&task_id, now - Duration::from_secs(count - index));
            task.task_status = if index % 2 == 0 { "completed" } else { "failed" }.to_string();
            seed_task(polly, &task);
            task_id
        })
        .collect();
    seeded.reverse();
    seeded
}

/// Milliseconds since the epoch, as a cursor carries them.
fn millis(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| {
            i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
        })
}

/// RULE: a continuation token is honoured only when the service ISSUED it — a value
/// that merely has the shape of one, whatever position or filter it names and whatever
/// authenticator it carries, is `InvalidNextTokenException` (HTTP 400) and reports no
/// tasks at all.
#[tokio::test]
async fn a_page_token_the_service_did_not_mint_is_refused() {
    let polly = Polly::new();
    let seeded = seed_tasks(&polly, 6);
    let now = millis(SystemTime::now());

    // A real token, so a stolen authenticator is in the table too: the tag of a token
    // the service DID mint, carried on a cursor it never minted.
    let minted = json(&list(&polly, "?MaxResults=2").await)["NextToken"]
        .as_str()
        .expect("six tasks do not fit in a two-task page")
        .to_string();
    let minted_bytes = ::hex::decode(&minted).expect("a minted token is hex");
    let stolen_tag = minted_bytes[..TAG_BYTES].to_vec();

    // Positions spanning the range a cursor can name, each with the authenticators a
    // caller can reach for.
    let positions: [(&str, i64); 6] = [
        ("the epoch, which would read as the first page", 0),
        ("the recent past", now - 60_000),
        ("this instant", now),
        ("the far future, which would read as an empty page", 99_999_999_999_999),
        ("before the epoch", -1),
        ("the extreme of the cursor's integer", i64::MAX),
    ];
    let tags: [(&str, Vec<u8>); 4] = [
        ("all-zero", vec![0; TAG_BYTES]),
        ("all-ones", vec![0xff; TAG_BYTES]),
        (
            "counted up",
            (0..TAG_BYTES).map(|byte| u8::try_from(byte % 256).unwrap_or_default()).collect(),
        ),
        ("stolen from a minted token", stolen_tag.clone()),
    ];

    let mut cases: Vec<(String, String)> = Vec::new();
    for (position, created_millis) in positions {
        for (tag_case, tag) in &tags {
            // No filter, and the two filters a cursor can claim it was minted under,
            // each presented with the matching `Status` so only the tag can refuse it.
            for (filter_case, status, query) in [
                ("no filter", "", String::new()),
                (
                    "a completed filter",
                    "completed",
                    "&Status=completed".to_string(),
                ),
                ("a failed filter", "failed", "&Status=failed".to_string()),
            ] {
                let token = forged_token(tag, created_millis, &seeded[0], status);
                cases.push((
                    format!("{position}, tag {tag_case}, {filter_case}"),
                    format!("?NextToken={token}{query}"),
                ));
            }
        }
        // The pre-authenticator shape: the bare cursor, with no tag at all.
        cases.push((
            format!("{position}, no authenticator at all"),
            format!(
                "?NextToken={}",
                ::hex::encode(format!(
                    "{created_millis}{CURSOR_SEPARATOR}{CURSOR_SEPARATOR}"
                ))
            ),
        ));
    }

    // A minted token with ONE byte changed, at every offset — the authenticator and
    // the cursor alike — plus one truncated and one extended.
    for index in 0..minted_bytes.len() {
        let mut tampered = minted_bytes.clone();
        tampered[index] ^= 0x01;
        cases.push((
            format!("a minted token with byte {index} of {} changed", minted_bytes.len()),
            format!("?NextToken={}", ::hex::encode(tampered)),
        ));
    }
    cases.push((
        "a minted token with its last byte dropped".to_string(),
        format!(
            "?NextToken={}",
            ::hex::encode(&minted_bytes[..minted_bytes.len() - 1])
        ),
    ));
    cases.push((
        "a minted token with a byte appended".to_string(),
        format!(
            "?NextToken={}",
            ::hex::encode([minted_bytes.as_slice(), b"x"].concat())
        ),
    ));
    // Shorter than an authenticator, not hex, and hex that is not UTF-8.
    for (case, token) in [
        ("hex too short to carry an authenticator", "00ff".to_string()),
        ("not hex at all", "not-a-token".to_string()),
        (
            "hex of bytes that are not UTF-8",
            ::hex::encode(vec![0xff_u8; TAG_BYTES + 8]),
        ),
    ] {
        cases.push((case.to_string(), format!("?NextToken={token}")));
    }

    for (case, query) in &cases {
        let response = list(&polly, query).await;
        let body = json(&response);
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "InvalidNextTokenException".to_string()),
            "case {case:?}: a token this service did not mint must be refused, got {body:?}"
        );
        assert!(
            body.get("SynthesisTasks").is_none(),
            "case {case:?}: a refused token must report no tasks, got {body:?}"
        );
    }

    // The control: the token the service DID mint still resumes its own listing, so the
    // check above refuses forgeries rather than every token.
    let resumed = list_ids(&polly, &format!("?MaxResults=2&NextToken={minted}")).await;
    assert_eq!(
        resumed,
        seeded[2..4].to_vec(),
        "a minted token must resume the listing it was issued for, got {resumed:?}"
    );
}

/// RULE: a token names a position in the report of the emulator that ISSUED it — one
/// minted by another emulator is refused, even when both hold the same tasks.
#[tokio::test]
async fn a_page_token_of_another_emulator_is_refused() {
    let issuer = Polly::new();
    let other = Polly::new();
    let seeded = seed_tasks(&issuer, 4);
    let also_seeded = seed_tasks(&other, 4);
    assert_eq!(
        seeded, also_seeded,
        "the two emulators must hold the same tasks for this to be about the token"
    );

    for max_results in [1usize, 2, 3] {
        let query = format!("?MaxResults={max_results}");
        let issued = json(&list(&issuer, &query).await)["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("MaxResults={max_results}: four tasks do not fit one page"))
            .to_string();

        // The emulator that issued it serves the page it names.
        let resumed = list_ids(&issuer, &format!("{query}&NextToken={issued}")).await;
        assert_eq!(
            resumed,
            seeded[max_results..(2 * max_results).min(seeded.len())].to_vec(),
            "MaxResults={max_results}: the issuer must resume its own token, got {resumed:?}"
        );

        // The other one, holding the very same tasks, did not issue it.
        let response = list(&other, &format!("{query}&NextToken={issued}")).await;
        let body = json(&response);
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "InvalidNextTokenException".to_string()),
            "MaxResults={max_results}: another emulator's token must be refused, got {body:?}"
        );
        let own_report = task_ids(&json(&list(&other, &query).await));
        assert_eq!(
            own_report.len(),
            max_results,
            "MaxResults={max_results}: the refusal must leave the other emulator's own report \
             intact, got {own_report:?}"
        );
    }
}
