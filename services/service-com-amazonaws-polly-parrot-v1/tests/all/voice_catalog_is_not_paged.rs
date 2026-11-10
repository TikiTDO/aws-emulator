// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RULE that a read the model declares no pagination for is
//! answered COMPLETE in one response: `DescribeVoices` reports every voice its query
//! keeps and sends no `NextToken`, so a client that calls it once and reads `Voices`
//! sees the whole catalog.
//!
//! The rule is checked over a TABLE of queries that spans the class the operation can
//! be asked about rather than the one unfiltered call an issue quotes: no filter (the
//! largest result there is), every modelled `Engine`, every modelled `LanguageCode`
//! with and without `IncludeAdditionalLanguageCodes`, both filters together, and a
//! query no voice satisfies (the empty result). The catalog the expectations are
//! derived from is the set of voices this service offers (`state::VOICE_CATALOG`),
//! every id of which is a value of the model's `VoiceId` enumeration.
//!
//! The last test is the boundary of the rule: the one operation this service models
//! WITH a pagination config still pages, so "no token" is a property of the model's
//! declaration and not of every list in the service.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_polly_parrot_v1::ops::types::{
    ENGINE_VALUES, LANGUAGE_CODE_VALUES, VOICE_ID_VALUES,
};
use service_com_amazonaws_polly_parrot_v1::state::VOICE_CATALOG;

// `voices` is the crate's one `DescribeVoices` reader: it makes a single call and
// asserts the response is complete (200, no `NextToken`), which is half of the rule
// under test here, so this suite drives it rather than standing up a second reader.
use crate::lexicons_and_speech::{error_code, ids, json, voices, Polly};
use crate::support::{seed_task, stored_task};

/// The queries the rule is checked over: no filter, every modelled engine, every
/// modelled language in both `IncludeAdditionalLanguageCodes` views, both filters
/// together, and a query no voice satisfies.
fn query_table() -> Vec<String> {
    let mut queries = vec![
        String::new(),
        "?IncludeAdditionalLanguageCodes=true".to_string(),
        // Welsh is a standard-only language, so crossing it with a generative engine is
        // a query the catalog answers with nothing.
        "?LanguageCode=cy-GB&Engine=generative".to_string(),
    ];
    for engine in ENGINE_VALUES {
        queries.push(format!("?Engine={engine}"));
        queries.push(format!("?Engine={engine}&LanguageCode=en-US"));
    }
    for language in LANGUAGE_CODE_VALUES {
        queries.push(format!("?LanguageCode={language}"));
        queries.push(format!(
            "?LanguageCode={language}&IncludeAdditionalLanguageCodes=true"
        ));
    }
    queries
}

/// RULE: one `DescribeVoices` call reports every voice its query keeps and sends no
/// `NextToken` — the model declares no pagination for this operation, so the response
/// is never truncated and the caller never has to loop.
#[tokio::test]
async fn one_call_reports_every_voice_the_query_keeps() {
    let polly = Polly::new();

    // The unfiltered call is the largest result the operation has, and it must carry
    // the whole catalog of voices the service offers, each id one the model declares.
    let all = voices(&polly, "").await;
    let reported = ids(&all);
    let offered: BTreeSet<String> = VOICE_CATALOG
        .iter()
        .map(|voice| voice.id.to_string())
        .collect();
    assert_eq!(
        reported, offered,
        "case \"unfiltered\": one call must report every offered voice, missing {:?}",
        offered.difference(&reported).collect::<Vec<_>>()
    );
    assert_eq!(
        all.len(),
        offered.len(),
        "case \"unfiltered\": {} rows for {} voices",
        all.len(),
        offered.len()
    );
    for id in &reported {
        assert!(
            VOICE_ID_VALUES.contains(&id.as_str()),
            "case \"unfiltered\": {id:?} is not a modelled VoiceId"
        );
    }

    // Every other query's complete answer, computed from that same catalog: what the
    // filters keep is a property of the voices, not of a page boundary.
    for query in query_table() {
        let reported = ids(&voices(&polly, &query).await);
        let expected = expected_for(&all, &query);
        assert_eq!(
            reported, expected,
            "case {query:?}: one call must report every voice the query keeps, {} reported \
             for {} expected",
            reported.len(),
            expected.len()
        );
    }
}

/// RULE: this service mints no continuation token for an unpaginated read, so a
/// `NextToken` presented to `DescribeVoices` is one it could not have issued and is
/// refused with `InvalidNextTokenException` (HTTP 400) rather than answered with part
/// of the catalog.
#[tokio::test]
async fn a_next_token_this_read_never_mints_is_refused() {
    let polly = Polly::new();

    // Tokens that differ in form: prose, padded base64, hex of a bare offset, hex of
    // the field-separated cursor shape this crate's paged read uses, and a token at the
    // modelled maximum length (`NextToken` is len 0..4096).
    let tokens: Vec<String> = vec![
        "not-a-token".to_string(),
        "this-was-never-issued-by-polly==".to_string(),
        "zzzz".to_string(),
        hex::encode("50"),
        hex::encode("50\u{1f}\u{1f}\u{1f}false"),
        hex::encode("0\u{1f}standard\u{1f}en-US\u{1f}true"),
        "a".repeat(4_096),
    ];
    // Each token under a bare query and under each filter, so no combination answers a
    // partial list.
    for token in &tokens {
        for filter in [
            "",
            "&LanguageCode=en-US",
            "&Engine=standard",
            "&IncludeAdditionalLanguageCodes=true",
        ] {
            let case = format!("?NextToken={token}{filter}");
            let response = polly.describe_voices(&case).await;
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "InvalidNextTokenException".to_string()),
                "case {case:?}: a token this read never mints must be refused, body {:?}",
                String::from_utf8_lossy(response.body())
            );
        }
    }

    // An empty token names no position and the model admits it (len 0..4096), so it
    // reads as no token at all: the complete catalog, with no token in return.
    let empty = voices(&polly, "?NextToken=").await;
    assert_eq!(
        ids(&empty).len(),
        VOICE_CATALOG.len(),
        "case \"?NextToken=\": an empty token must read as no token, got {} voices",
        empty.len()
    );
}

/// RULE (the boundary): an operation the model DOES declare a pagination config for
/// still pages — `ListSpeechSynthesisTasks` carries `MaxResults` and mints a
/// `NextToken` while further tasks exist.
#[tokio::test]
async fn the_operation_the_model_paginates_still_pages() {
    let polly = Polly::new();
    let now = SystemTime::now();
    let total = 5u64;
    for index in 0..total {
        seed_task(
            &polly,
            &stored_task(
                &format!("Task{index:02}"),
                now - Duration::from_secs(total - index),
            ),
        );
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let query = match &token {
            Some(next) => format!("?MaxResults=2&NextToken={next}"),
            None => "?MaxResults=2".to_string(),
        };
        let response = polly.call("GET", &format!("/v1/synthesisTasks{query}"), "").await;
        assert_eq!(
            response.status(),
            200,
            "case {query:?}: ListSpeechSynthesisTasks must succeed, body {:?}",
            String::from_utf8_lossy(response.body())
        );
        let body = json(&response);
        let page: Vec<String> = body["SynthesisTasks"]
            .as_array()
            .unwrap_or_else(|| panic!("case {query:?}: SynthesisTasks must be a list"))
            .iter()
            .map(|task| task["TaskId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            page.len() <= 2,
            "case {query:?}: a page must carry at most MaxResults tasks, got {}",
            page.len()
        );
        seen.extend(page);
        pages += 1;
        match body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(pages < 10, "case {query:?}: pagination did not terminate");
    }
    assert!(
        pages > 1,
        "the paginated read must truncate at MaxResults, it answered in {pages} page(s)"
    );
    assert_eq!(
        seen.len(),
        total as usize,
        "the pages together must be every task once, got {seen:?}"
    );
}

/// The voices a query keeps, selected from the complete catalog by the rules the
/// filters document: `Engine` keeps voices that support it, `LanguageCode` keeps voices
/// whose default language it is, and counts a bilingual voice's additional languages
/// only when `IncludeAdditionalLanguageCodes` is set.
fn expected_for(all: &[serde_json::Value], query: &str) -> BTreeSet<String> {
    let parameter = |name: &str| -> Option<String> {
        query
            .trim_start_matches('?')
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    };
    let engine = parameter("Engine");
    let language = parameter("LanguageCode");
    let include_additional = parameter("IncludeAdditionalLanguageCodes").as_deref() == Some("true");

    all.iter()
        .filter(|voice| {
            engine.as_deref().is_none_or(|wanted| {
                voice["SupportedEngines"]
                    .as_array()
                    .is_some_and(|engines| engines.iter().any(|e| e.as_str() == Some(wanted)))
            })
        })
        .filter(|voice| {
            language.as_deref().is_none_or(|wanted| {
                voice["LanguageCode"].as_str() == Some(wanted)
                    || (include_additional
                        && voice["AdditionalLanguageCodes"]
                            .as_array()
                            .is_some_and(|codes| {
                                codes.iter().any(|code| code.as_str() == Some(wanted))
                            }))
            })
        })
        .map(|voice| voice["Id"].as_str().unwrap_or_default().to_string())
        .collect()
}
