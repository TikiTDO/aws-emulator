// SPDX-License-Identifier: CC0-1.0

//! Listing and deleting Kinesis streams, end to end over the wire: `CreateStream` ->
//! `ListStreams` -> `DeleteStream`.
//!
//! RULE: `ListStreams` reports the streams the addressed region actually holds, in name
//! order, one page of at most the request's `Limit` at a time, resumed at the stream its
//! `ExclusiveStartStreamName` or its `NextToken` names, with `HasMoreStreams` true exactly
//! when the region holds a stream past the page; `DeleteStream` moves the stream it names to
//! `DELETING` and the stream then stops naming a resource for every operation — its name is
//! free again, and a read of it answers `ResourceNotFoundException`.
//!
//! The flow this was derived from lists the streams of one account and deletes one of them;
//! it is one row of the tables below. The rows differ along what the rule generalises over:
//! how many streams the region holds, which page size the caller asks for, which cursor
//! resumes the listing, which identifier the delete addresses the stream by, and the status
//! the stream is in when the call arrives.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    next_token_expired_message, shard_id, stream_not_found_message, RegionalListing,
    RegionalPageToken,
    DELETION_DELAY, MAX_LIST_STREAMS_LIMIT, PAGE_TOKEN_TTL, SHARD_ITERATOR_TYPE_TRIM_HORIZON,
    STREAM_ACTIVE, STREAM_DELETING,
};

// The one wire harness this crate's suites share — a router carrying exactly this service
// over one shared engine, with the request-signing and error-body helpers around it. The
// listing and deletion steps this suite needs are the functions below rather than a second
// copy of it.
use crate::stream_tags::{error_code, error_message, past_activation, Emulator, REGION};

/// A second region, so a case can show that a listing reports the streams of the region the
/// request was signed for and no other.
const OTHER_REGION: &str = "eu-west-1";

/// How long a case waits before treating a deleted stream as gone: past the instant the
/// delete armed the `DELETING` -> gone transition for.
fn past_deletion() -> Duration {
    DELETION_DELAY + Duration::from_millis(400)
}

/// Call `operation` expecting a client refusal, answering its error code and message.
async fn refused(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
) -> (String, String) {
    let (status, response) = emulator.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected {operation} to answer 400, got {status} with body {response}"
    );
    (error_code(&response), error_message(&response))
}

/// A provisioned stream of one shard in `region`, past the transition the create armed,
/// answering its `StreamARN`.
async fn active_stream_in(emulator: &Emulator, case: &str, region: &str, name: &str) -> String {
    emulator
        .ok_in(
            case,
            region,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let summary = emulator
        .ok_in(
            case,
            region,
            "DescribeStreamSummary",
            &json!({"StreamName": name}),
        )
        .await;
    summary["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: DescribeStreamSummary reported no StreamARN"))
        .to_string()
}

/// One `ListStreams` page: the names it reported, whether more are available and the cursor
/// to continue at.
async fn list_streams(
    emulator: &Emulator,
    case: &str,
    body: &Value,
) -> (Vec<String>, bool, Option<String>) {
    let answer = emulator.ok(case, "ListStreams", body).await;
    let names = answer["StreamNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: ListStreams answered no StreamNames"))
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    let has_more = answer["HasMoreStreams"]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case}: HasMoreStreams is required, got {answer}"));
    let next = answer["NextToken"].as_str().map(str::to_string);
    (names, has_more, next)
}

/// The `StreamStatus` `DescribeStreamSummary` reports for `name`, or the error code it
/// refused the read under.
async fn stream_status(emulator: &Emulator, name: &str) -> Result<String, String> {
    let (status, response) = emulator
        .call("DescribeStreamSummary", &json!({"StreamName": name}))
        .await;
    if status == 200 {
        Ok(response["StreamDescriptionSummary"]["StreamStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    } else {
        Err(error_code(&response))
    }
}

/// The `Data` member as it travels on the wire: a blob is base64-encoded when it is
/// serialized.
fn data(bytes: &[u8]) -> Value {
    json!(aws_smithy_types::base64::encode(bytes))
}

/// A listing reports the streams the region holds in name order, one page at a time, for
/// every page size and cursor the model admits.
#[tokio::test]
async fn a_listing_reports_the_regions_streams_in_name_order_one_page_at_a_time() {
    let emulator = Emulator::new();
    // Created out of order, so the order a page reports is the one the listing imposes and
    // not the order of creation.
    let created = ["mid-stream", "alpha-stream", "zeta-stream", "beta-stream"];
    for name in created {
        active_stream_in(&emulator, "listing", REGION, name).await;
    }
    // A stream of another region, which this region's listing never reports.
    active_stream_in(&emulator, "listing", OTHER_REGION, "other-region-stream").await;
    let mut all: Vec<String> = created.iter().map(|name| (*name).to_string()).collect();
    all.sort();

    // (case, the page size the request names, how many names the page holds)
    let cases: [(&str, Option<i64>, usize); 6] = [
        ("no Limit", None, 4),
        ("one stream a page", Some(1), 1),
        ("three streams a page", Some(3), 3),
        ("exactly as many as the region holds", Some(4), 4),
        // One past the page ceiling `ListStreamsInput$Limit` documents, and the modelled
        // maximum the member's `range 1..10000` admits: both are clamped to that ceiling,
        // which is well above what this region holds.
        (
            "one past the documented page ceiling",
            Some(i64::from(MAX_LIST_STREAMS_LIMIT) + 1),
            4,
        ),
        ("the modelled maximum page", Some(10_000), 4),
    ];
    for (case, limit, expected_len) in cases {
        let mut body = json!({});
        if let Some(limit) = limit {
            body["Limit"] = json!(limit);
        }
        let (names, has_more, next) = list_streams(&emulator, case, &body).await;
        assert_eq!(
            names,
            all[..expected_len].to_vec(),
            "case {case}: expected the first {expected_len} streams in name order, got {names:?}"
        );
        assert_eq!(
            has_more,
            expected_len < all.len(),
            "case {case}: HasMoreStreams is true exactly when a stream is left past the page"
        );
        assert_eq!(
            next.is_some(),
            expected_len < all.len(),
            "case {case}: a cursor is handed back exactly when the page is truncated"
        );

        // Paging all the way through with that page size reports every stream once, in the
        // same order, whichever cursor resumes it.
        for cursor_kind in ["ExclusiveStartStreamName", "NextToken"] {
            let row = format!("{case} / resumed by {cursor_kind}");
            let mut collected: Vec<String> = Vec::new();
            let mut cursor: Option<String> = None;
            loop {
                let mut page = json!({});
                if let Some(limit) = limit {
                    page["Limit"] = json!(limit);
                }
                if let Some(cursor) = cursor.as_deref() {
                    page[cursor_kind] = json!(cursor);
                }
                let (names, has_more, next) = list_streams(&emulator, &row, &page).await;
                collected.extend(names.iter().cloned());
                if !has_more {
                    assert!(
                        next.is_none(),
                        "case {row}: the last page hands back no cursor, got {next:?}"
                    );
                    break;
                }
                cursor = Some(if cursor_kind == "NextToken" {
                    next.unwrap_or_else(|| panic!("case {row}: a truncated page hands back a token"))
                } else {
                    names
                        .last()
                        .cloned()
                        .unwrap_or_else(|| panic!("case {row}: a truncated page holds a name"))
                });
                assert!(
                    collected.len() <= all.len(),
                    "case {row}: paging must terminate, got {collected:?}"
                );
            }
            assert_eq!(
                collected, all,
                "case {row}: paging reports every stream of the region exactly once, in name \
                 order"
            );
        }
    }

    // The summaries a page carries describe the same streams it named.
    let answer = emulator.ok("summaries", "ListStreams", &json!({})).await;
    let summaries = answer["StreamSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case summaries: expected StreamSummaries, got {answer}"));
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary["StreamName"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<String>>(),
        all,
        "case summaries: the summaries describe the streams the page named, in the same order"
    );
    for summary in summaries {
        let name = summary["StreamName"].as_str().unwrap_or_default();
        assert_eq!(
            summary["StreamStatus"].as_str(),
            Some(STREAM_ACTIVE),
            "case summaries: {name} is ACTIVE, got {summary}"
        );
        assert!(
            summary["StreamARN"]
                .as_str()
                .is_some_and(|arn| arn.ends_with(&format!(":stream/{name}"))),
            "case summaries: {name} carries its own ARN, got {summary}"
        );
        assert!(
            summary["StreamCreationTimestamp"].is_number(),
            "case summaries: {name} carries the instant it was created, got {summary}"
        );
    }

    // An account that holds no stream in the addressed region lists none.
    let empty = Emulator::new();
    let (names, has_more, next) = list_streams(&empty, "empty", &json!({})).await;
    assert!(
        names.is_empty() && !has_more && next.is_none(),
        "case empty: a region holding no stream lists none, got {names:?} / {has_more} / \
         {next:?}"
    );
}

/// A cursor this service did not issue, one past its lifetime and one minted for another
/// region each name no page, and a cursor cannot be combined with the member it replaces.
#[tokio::test]
async fn a_cursor_no_page_of_this_listing_was_handed_out_for_is_refused() {
    let emulator = Emulator::new();
    for name in ["cursor-alpha", "cursor-beta"] {
        active_stream_in(&emulator, "cursors", REGION, name).await;
    }

    let expired = RegionalPageToken::new(
        RegionalListing::Streams,
        REGION,
        "cursor-beta",
        SystemTime::now() - PAGE_TOKEN_TTL - Duration::from_secs(1),
    )
    .encode();
    let other_region = RegionalPageToken::new(
        RegionalListing::Streams,
        OTHER_REGION,
        "cursor-beta",
        SystemTime::now(),
    )
    .encode();
    let live = RegionalPageToken::new(
        RegionalListing::Streams,
        REGION,
        "cursor-beta",
        SystemTime::now(),
    )
    .encode();

    // (case, the request, the modelled error code it is refused under)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a token this service never issued",
            json!({"NextToken": "not-a-token"}),
            "InvalidArgumentException",
        ),
        (
            "a token past its lifetime",
            json!({"NextToken": expired}),
            "ExpiredNextTokenException",
        ),
        (
            "a token minted for another region",
            json!({"NextToken": other_region}),
            "InvalidArgumentException",
        ),
        (
            "a token beside the member it replaces",
            json!({"NextToken": live, "ExclusiveStartStreamName": "cursor-alpha"}),
            "InvalidArgumentException",
        ),
    ];
    for (case, body, expected) in cases {
        let (code, message) = refused(&emulator, case, "ListStreams", &body).await;
        assert_eq!(
            code, expected,
            "case {case}: expected {expected}, got {code} with message {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case}: a refusal carries a message, got none"
        );
    }

    let expired_again = RegionalPageToken::new(
        RegionalListing::Streams,
        REGION,
        "cursor-beta",
        SystemTime::now() - PAGE_TOKEN_TTL - Duration::from_secs(1),
    )
    .encode();
    let (_, message) = refused(
        &emulator,
        "the expiry sentence",
        "ListStreams",
        &json!({"NextToken": expired_again}),
    )
    .await;
    assert_eq!(
        message,
        next_token_expired_message(),
        "a cursor past its lifetime is refused with the service's one pagination-expiry \
         sentence"
    );
}

/// A deleted stream is reported `DELETING` until the removal the delete armed falls due, and
/// from then on it names no resource: no read of it is served, it is listed no more and its
/// name is free again.
#[tokio::test]
async fn a_deleted_stream_stops_naming_a_resource() {
    // (case, how the delete addresses the stream)
    for case in ["by StreamName", "by StreamARN"] {
        let emulator = Emulator::new();
        let stream = "deleted-stream";
        let kept = "kept-stream";
        let arn = active_stream_in(&emulator, case, REGION, stream).await;
        active_stream_in(&emulator, case, REGION, kept).await;

        // A record and a reader, so the delete is shown to take the shard log with it.
        emulator
            .ok(
                case,
                "PutRecord",
                &json!({"StreamName": stream, "PartitionKey": "pk", "Data": data(b"gone soon")}),
            )
            .await;
        let iterator = emulator
            .ok(
                case,
                "GetShardIterator",
                &json!({
                    "StreamName": stream,
                    "ShardId": shard_id(0),
                    "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
                }),
            )
            .await["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a ShardIterator"))
            .to_string();

        let delete_body = if case == "by StreamName" {
            json!({"StreamName": stream})
        } else {
            json!({"StreamARN": arn})
        };
        emulator.ok(case, "DeleteStream", &delete_body).await;

        // Straight after the call the stream is still held, in the status the delete put it
        // in, and a second delete of it is refused for not being ACTIVE.
        assert_eq!(
            stream_status(&emulator, stream).await,
            Ok(STREAM_DELETING.to_string()),
            "case {case}: a stream a delete was accepted for reports DELETING"
        );
        let (code, message) = refused(&emulator, case, "DeleteStream", &delete_body).await;
        assert_eq!(
            code, "ResourceInUseException",
            "case {case}: deleting a stream that is not ACTIVE is refused, got {code} with \
             message {message:?}"
        );

        tokio::time::sleep(past_deletion()).await;

        // From the instant the removal falls due the stream names no resource at all.
        assert_eq!(
            stream_status(&emulator, stream).await,
            Err("ResourceNotFoundException".to_string()),
            "case {case}: a removed stream is read as a resource this account does not hold"
        );
        let (names, _, _) = list_streams(&emulator, case, &json!({})).await;
        assert_eq!(
            names,
            vec![kept.to_string()],
            "case {case}: a removed stream is listed no more, and every other stream still is"
        );
        // (case, the operation a client tries on the removed stream)
        let gone: Vec<(&str, &str, Value)> = vec![
            (
                "describing it",
                "DescribeStreamSummary",
                json!({"StreamName": stream}),
            ),
            (
                "writing to it",
                "PutRecord",
                json!({"StreamName": stream, "PartitionKey": "pk", "Data": data(b"x")}),
            ),
            (
                "writing a batch to it",
                "PutRecords",
                json!({
                    "StreamName": stream,
                    "Records": [{"PartitionKey": "pk", "Data": data(b"x")}],
                }),
            ),
            (
                "taking an iterator in it",
                "GetShardIterator",
                json!({
                    "StreamName": stream,
                    "ShardId": shard_id(0),
                    "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
                }),
            ),
            (
                "reading an iterator it handed out",
                "GetRecords",
                json!({"ShardIterator": iterator}),
            ),
            ("deleting it again", "DeleteStream", delete_body.clone()),
            ("listing its shards", "ListShards", json!({"StreamName": stream})),
            (
                "listing its tags",
                "ListTagsForStream",
                json!({"StreamName": stream}),
            ),
        ];
        for (row, operation, body) in gone {
            let (code, message) =
                refused(&emulator, &format!("{case} / {row}"), operation, &body).await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case} / {row}: expected ResourceNotFoundException, got {code} with \
                 message {message:?}"
            );
        }

        // The name is free again: a create takes it, and the new stream is a stream of its
        // own with an empty shard log.
        active_stream_in(&emulator, case, REGION, stream).await;
        let fresh = emulator
            .ok(
                case,
                "GetShardIterator",
                &json!({
                    "StreamName": stream,
                    "ShardId": shard_id(0),
                    "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
                }),
            )
            .await["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a ShardIterator"))
            .to_string();
        let answer = emulator
            .ok(case, "GetRecords", &json!({"ShardIterator": fresh}))
            .await;
        assert_eq!(
            answer["Records"].as_array().map(Vec::len),
            Some(0),
            "case {case}: the stream that took the name holds none of the deleted stream's \
             records, got {answer}"
        );
    }
}

/// Deleting a stream this account and region do not hold, or naming no stream at all, is
/// refused rather than answered.
#[tokio::test]
async fn deleting_a_stream_nothing_created_is_refused() {
    let emulator = Emulator::new();
    let arn = active_stream_in(&emulator, "refusals", REGION, "delete-refusals").await;

    // (case, the request, the modelled error code it is refused under)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a stream this account and region do not hold",
            json!({"StreamName": "delete-no-such-stream"}),
            "ResourceNotFoundException",
        ),
        (
            "an ARN no stream carries",
            json!({"StreamARN": "arn:aws:kinesis:us-east-1:000000000000:stream/delete-nothing"}),
            "ResourceNotFoundException",
        ),
        (
            "neither StreamName nor StreamARN",
            json!({}),
            "InvalidArgumentException",
        ),
        (
            "a StreamName and StreamARN that name different streams",
            json!({"StreamName": "delete-another-stream", "StreamARN": arn}),
            "InvalidArgumentException",
        ),
        (
            "a stream of another region",
            json!({"StreamName": "delete-refusals", "StreamARN": arn.replace(REGION, OTHER_REGION)}),
            "ResourceNotFoundException",
        ),
    ];
    for (case, body, expected) in cases {
        let (code, message) = refused(&emulator, case, "DeleteStream", &body).await;
        assert_eq!(
            code, expected,
            "case {case}: expected {expected}, got {code} with message {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case}: a refusal carries a message, got none"
        );
    }

    let (_, message) = refused(
        &emulator,
        "the missing stream is named",
        "DeleteStream",
        &json!({"StreamName": "delete-no-such-stream"}),
    )
    .await;
    assert_eq!(
        message,
        stream_not_found_message("delete-no-such-stream"),
        "a missing stream is refused with the service's stream-not-found sentence"
    );

    // A stream whose creation has not fallen due yet is deleted all the same: the removal
    // does not wait for the stream to reach ACTIVE, it only needs a stream this account
    // holds whose removal has not already been accepted.
    emulator
        .ok(
            "creating",
            "CreateStream",
            &json!({"StreamName": "delete-creating", "ShardCount": 1}),
        )
        .await;
    emulator
        .ok(
            "creating",
            "DeleteStream",
            &json!({"StreamName": "delete-creating"}),
        )
        .await;
    assert_eq!(
        stream_status(&emulator, "delete-creating").await,
        Ok(STREAM_DELETING.to_string()),
        "case creating: a stream a delete was accepted for reports DELETING"
    );
    // The accepted delete is the only one: a second request has nothing left to accept.
    let (code, message) = refused(
        &emulator,
        "creating",
        "DeleteStream",
        &json!({"StreamName": "delete-creating"}),
    )
    .await;
    assert_eq!(
        code, "ResourceInUseException",
        "case creating: a stream already DELETING is refused, got {code} with message \
         {message:?}"
    );
    tokio::time::sleep(past_deletion()).await;
    let (code, _) = refused(
        &emulator,
        "creating",
        "DeleteStream",
        &json!({"StreamName": "delete-creating"}),
    )
    .await;
    assert_eq!(
        code, "ResourceNotFoundException",
        "case creating: once the removal falls due the stream names no resource at all"
    );
}
