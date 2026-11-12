// SPDX-License-Identifier: CC0-1.0

//! The Kinesis CHANNEL LISTING, end to end over the wire: `CreateChannel` -> `ListChannels`.
//!
//! RULE: `ListChannels` REPORTS the channels this account and region hold, computed from the
//! records `CreateChannel` persisted and writing nothing of its own. Each channel is projected
//! onto `ChannelSummary` — the name, ARN and id it is addressed by, its creation instant, the
//! status the clock has reached (`CREATING` until the transition the create armed falls due,
//! `ACTIVE` from then on), the destination kind it delivers to, and its source streams as the
//! ARN / creation-instant pair a summary carries — so a summary never disagrees with the
//! `ChannelDescription` a `DescribeChannel` of the same channel answers with. The channels are
//! ordered by name whatever order they were created in, a page holds at most `MaxResults` of
//! them (100 by default, and at most 100), and a `NextToken` is handed back exactly when the
//! region holds a channel past the page, which resumes precisely where the page stopped and is
//! refused once it no longer names a page — with `InvalidArgumentException` and "Invalid
//! NextToken." when it is not a cursor this listing issued (it does not decode and verify, or it
//! was minted for another listing or another region), and with `ExpiredNextTokenException` only
//! when a cursor it DID issue is outside its lifetime. A `StreamFilter` keeps only the channels
//! one of whose source
//! streams it names, and the listing is scoped to the region the request was signed for.
//!
//! The recorded flow this was derived from creates ONE channel and lists it, which is one row
//! of the tables below. The rows differ along what the rule generalises over: how many channels
//! the region holds, the order they were created in, the destination kind each delivers to, the
//! page size the request asks for (including each published bound of `MaxResults` and one past
//! it), how many source streams the filter names and whether it pins their creation instants,
//! the lifecycle status the clock has reached, and the region the request is signed for.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    channel_arn, next_token_expired_message, RegionalListing, RegionalPageToken,
    INVALID_NEXT_TOKEN_MESSAGE, MAX_LIST_CHANNELS_LIMIT, PAGE_TOKEN_TTL,
};

use crate::channel_lifecycle::{
    bucket_arn, dead_letter_queue, description, member, on_demand_stream, on_demand_stream_in,
    past_channel_activation, role_arn, schema_arn, storage_configuration, stream_configuration,
    table_bucket_arn,
};
use crate::stream_tags::{error_code, error_message, Emulator, REGION};

/// The status a freshly created channel reports: "CREATING - The channel is being created"
/// (`smithy_query(query="shape",shape="ChannelSummary")` -> `ChannelStatus`).
const CREATING: &str = "CREATING";

/// The status a provisioned channel reports: "ACTIVE - The channel is ready to deliver
/// records" (same member).
const ACTIVE: &str = "ACTIVE";

/// The destination kind of a channel delivering to a general purpose bucket: "S3 - Delivery to
/// a general purpose Amazon S3 bucket" (same call -> `ChannelDestinationType`).
const DESTINATION_S3: &str = "S3";

/// The destination kind of a channel delivering to streaming tables: "S3_TABLES - Delivery to
/// streaming tables on Apache Iceberg" (same member).
const DESTINATION_S3_TABLES: &str = "S3_TABLES";

/// The modelled maximum of `MaxResults`
/// (`smithy_query(query="operation",operation="ListChannels")` -> `MaxResults` range 1..10000).
const MAX_RESULTS_CEILING: i64 = 10_000;

/// Create a channel named `name` in `region`, sourcing `source` and delivering to a general
/// purpose bucket. Answers the `ChannelDescription` the create reported.
async fn s3_channel(emulator: &Emulator, region: &str, name: &str, source: &str) -> Value {
    let created = emulator
        .ok_in(
            name,
            region,
            "CreateChannel",
            &json!({
                "ChannelName": name,
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(source, "JSON", None)],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration(&format!("bucket-{name}")),
                },
            }),
        )
        .await;
    description(name, &created)
}

/// Create a channel named `name` in the default region, sourcing `source` and delivering to
/// streaming tables. Answers the `ChannelDescription` the create reported.
async fn tables_channel(emulator: &Emulator, name: &str, source: &str) -> Value {
    let created = emulator
        .ok(
            name,
            "CreateChannel",
            &json!({
                "ChannelName": name,
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(
                    source,
                    "GSR_JSON",
                    Some(schema_arn("records", "iceberg")),
                )],
                "S3TablesDestinationConfiguration": {
                    "DeadLetterQueueS3Configuration": dead_letter_queue("channel-dlq", "tables/"),
                    "S3TablesConfigurationList": [{
                        "TableBucketARN": table_bucket_arn("channel-tables"),
                        "Namespace": "events",
                        "TableName": "records",
                        "CompressionType": "SNAPPY",
                    }],
                },
            }),
        )
        .await;
    description(name, &created)
}

/// The `ChannelSummaries` a `ListChannels` response carries, which the model makes required.
fn summaries(case: &str, response: &Value) -> Vec<Value> {
    response
        .get("ChannelSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case}: ListChannels must carry ChannelSummaries, got {response}")
        })
        .clone()
}

/// The channel names a listing reported, in the order it reported them.
fn listed_names(case: &str, response: &Value) -> Vec<String> {
    summaries(case, response)
        .iter()
        .map(|summary| member(case, summary, "ChannelName"))
        .collect()
}

/// The `NextToken` a listing handed back, absent when the body carries none or null.
fn next_token(response: &Value) -> Option<String> {
    response
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The summary of the channel named `name`, which the listing must carry.
fn summary_of(case: &str, response: &Value, name: &str) -> Value {
    summaries(case, response)
        .into_iter()
        .find(|summary| member(case, summary, "ChannelName") == name)
        .unwrap_or_else(|| panic!("case {case}: the listing must carry {name}, got {response}"))
}

/// The summary with the status taken out: the status is the one member the clock moves, so this
/// is what two listings of one channel must agree on.
fn summary_without_status(case: &str, response: &Value, name: &str) -> Value {
    let mut summary = summary_of(case, response, name);
    if let Some(members) = summary.as_object_mut() {
        members.remove("ChannelStatus");
    }
    summary
}

/// Page through `ListChannels` from `body`, following `NextToken` to the end, and answer every
/// channel name in the order the pages reported them. Each page but the last must be full, and
/// a page that hands back a cursor must carry channels.
async fn every_page(emulator: &Emulator, case: &str, body: &Value, page_size: usize) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    // One page per channel the listing could hold, plus the call that ends it: a listing that
    // keeps handing out cursors past that is looping rather than paging.
    for _ in 0..=(MAX_LIST_CHANNELS_LIMIT as usize) {
        let mut page_body = body.clone();
        if let Some(token) = token.take() {
            page_body["NextToken"] = json!(token);
        }
        let answer = emulator.ok(case, "ListChannels", &page_body).await;
        let page = listed_names(case, &answer);
        assert!(
            page.len() <= page_size,
            "case {case}: expected at most {page_size} channels per page, got {page:?}"
        );
        names.extend(page.iter().cloned());
        match next_token(&answer) {
            None => return names,
            Some(next) => {
                assert_eq!(
                    page.len(),
                    page_size,
                    "case {case}: a page that hands back a NextToken must be full, got {page:?}"
                );
                token = Some(next);
            }
        }
    }
    panic!("case {case}: ListChannels never stopped handing out a NextToken");
}

/// One row of the projection table: a stored channel, the destination kind its summary must
/// report, and the `ChannelDescription` the create answered with.
struct ProjectionRow {
    name: &'static str,
    destination: &'static str,
    description: Value,
}

// 1. Every stored channel of the region, projected onto `ChannelSummary` and ordered by name
#[tokio::test]
async fn a_listing_projects_every_stored_channel_of_the_region() {
    let emulator = Emulator::new();
    let one = on_demand_stream(&emulator, "listing-source-one").await;
    let two = on_demand_stream(&emulator, "listing-source-two").await;

    // Created out of alphabetical order, so the order a listing reports is the rule's and not
    // the one the creates happened in. The rows differ along the destination kind and which
    // stream each channel sources.
    let rows = vec![
        ProjectionRow {
            name: "gamma-channel",
            destination: DESTINATION_S3,
            description: s3_channel(&emulator, REGION, "gamma-channel", &two).await,
        },
        ProjectionRow {
            name: "alpha-channel",
            destination: DESTINATION_S3,
            description: s3_channel(&emulator, REGION, "alpha-channel", &one).await,
        },
        ProjectionRow {
            name: "beta-channel",
            destination: DESTINATION_S3_TABLES,
            description: tables_channel(&emulator, "beta-channel", &two).await,
        },
    ];

    let case = "projection";
    let listed = emulator.ok(case, "ListChannels", &json!({})).await;
    assert_eq!(
        listed_names(case, &listed),
        vec!["alpha-channel", "beta-channel", "gamma-channel"],
        "case {case}: expected every channel of {REGION} ordered by name, got {listed}"
    );
    assert_eq!(
        next_token(&listed),
        None,
        "case {case}: a listing that fits one page carries no NextToken, got {listed}"
    );

    for row in &rows {
        let summary = summary_of(case, &listed, row.name);
        let description = &row.description;
        for shared in [
            "ChannelName",
            "ChannelARN",
            "ChannelId",
            "ChannelCreationTimestamp",
        ] {
            assert_eq!(
                summary.get(shared),
                description.get(shared),
                "case {case}/{}: expected the summary's {shared} to be the stored record's, \
                 got {summary} beside {description}",
                row.name
            );
        }
        assert_eq!(
            member(row.name, &summary, "ChannelARN"),
            channel_arn(REGION, row.name),
            "case {case}/{}: expected the ARN of the channel created in {REGION}, got {summary}",
            row.name
        );
        assert_eq!(
            member(row.name, &summary, "ChannelDestinationType"),
            row.destination,
            "case {case}/{}: expected the destination kind the create named, got {summary}",
            row.name
        );
        // A channel this emulator holds provisions, so none of them is FAILED and none carries
        // a reason ("A message describing the reason for a FAILED status").
        assert_eq!(
            summary.get("ChannelStatusReason"),
            None,
            "case {case}/{}: a channel that did not fail carries no status reason, got {summary}",
            row.name
        );
        assert_eq!(
            member(row.name, &summary, "ChannelStatus"),
            CREATING,
            "case {case}/{}: expected a freshly created channel to report {CREATING}, got \
             {summary}",
            row.name
        );
        // The summary reports its source streams as the identifier pair, taken from the same
        // stored streams the description reports in full.
        let sources: Vec<Value> = description
            .get("StreamConfigurationList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case}/{}: the description must carry its streams", row.name)
            })
            .iter()
            .map(|stream| {
                json!({
                    "StreamARN": stream.get("StreamARN"),
                    "StreamCreationTimestamp": stream.get("StreamCreationTimestamp"),
                })
            })
            .collect();
        assert_eq!(
            summary.get("Streams").cloned().unwrap_or(Value::Null),
            Value::Array(sources.clone()),
            "case {case}/{}: expected the stored source streams as identifier pairs, got \
             {summary}",
            row.name
        );
    }

    // A listing is a read: listing twice without anything else happening answers the same
    // thing, so nothing a later caller sees was changed by the first call.
    let again = emulator.ok(case, "ListChannels", &json!({})).await;
    assert_eq!(
        again, listed,
        "case {case}: expected two listings at the same instant to agree, got {again} after \
         {listed}"
    );

    // Once the transition the creates armed falls due, the status is the only member that
    // moved.
    tokio::time::sleep(past_channel_activation()).await;
    let activated = emulator.ok(case, "ListChannels", &json!({})).await;
    for row in &rows {
        assert_eq!(
            member(row.name, &summary_of(case, &activated, row.name), "ChannelStatus"),
            ACTIVE,
            "case {case}/{}: expected {ACTIVE} once the transition falls due, got {activated}",
            row.name
        );
        assert_eq!(
            summary_without_status(case, &activated, row.name),
            summary_without_status(case, &listed, row.name),
            "case {case}/{}: expected the clock to move the status and nothing else, got \
             {activated}",
            row.name
        );
    }
}

/// One row of the paging table: the page size a request asks for, and the page size the rule
/// clamps it to.
struct PageRow {
    case: &'static str,
    max_results: Option<i64>,
    page_size: usize,
}

// 2. `MaxResults` bounds a page, and `NextToken` resumes exactly where it stopped
#[tokio::test]
async fn a_listing_pages_by_max_results() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "paging-source").await;
    let held = ["page-a", "page-b", "page-c", "page-d", "page-e"];
    for name in held {
        s3_channel(&emulator, REGION, name, &source).await;
    }
    let expected: Vec<String> = held.iter().map(|name| name.to_string()).collect();

    let rows = [
        PageRow {
            case: "one-at-a-time",
            max_results: Some(1),
            page_size: 1,
        },
        PageRow {
            case: "two-at-a-time",
            max_results: Some(2),
            page_size: 2,
        },
        PageRow {
            case: "exactly-the-held-channels",
            max_results: Some(held.len() as i64),
            page_size: held.len(),
        },
        PageRow {
            case: "no-max-results-at-all",
            max_results: None,
            page_size: MAX_LIST_CHANNELS_LIMIT as usize,
        },
        PageRow {
            // "If you specify a value greater than 100, at most 100 results are returned"
            // (`ListChannelsInput$MaxResults`), so the modelled ceiling is clamped to 100.
            case: "past-the-served-ceiling",
            max_results: Some(MAX_RESULTS_CEILING),
            page_size: MAX_LIST_CHANNELS_LIMIT as usize,
        },
    ];

    for row in &rows {
        let mut body = json!({});
        if let Some(max_results) = row.max_results {
            body["MaxResults"] = json!(max_results);
        }
        let paged = every_page(&emulator, row.case, &body, row.page_size).await;
        assert_eq!(
            paged, expected,
            "case {}: expected the pages to report every channel of {REGION} in name order, \
             got {paged:?}",
            row.case
        );
    }

    // A cursor resumes AT the channel it names: the first page's own channels are not repeated
    // and none is skipped.
    let case = "resumed";
    let first = emulator
        .ok(case, "ListChannels", &json!({ "MaxResults": 2 }))
        .await;
    let token = next_token(&first)
        .unwrap_or_else(|| panic!("case {case}: a truncated listing hands back a NextToken"));
    let resumed = emulator
        .ok(case, "ListChannels", &json!({ "NextToken": token }))
        .await;
    assert_eq!(
        listed_names(case, &resumed),
        expected[2..].to_vec(),
        "case {case}: expected the cursor to resume at the channel it names, got {resumed}"
    );
}

// 2b. Each published bound of `MaxResults` is served and a value one past either is refused
#[tokio::test]
async fn a_max_results_outside_its_published_range_is_refused() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "range-source").await;
    s3_channel(&emulator, REGION, "range-channel", &source).await;

    // `MaxResults` is modelled `range 1..10000`, so each published bound is served.
    for max_results in [1, MAX_RESULTS_CEILING] {
        emulator
            .ok(
                "max-results-inside-the-range",
                "ListChannels",
                &json!({ "MaxResults": max_results }),
            )
            .await;
    }
    for max_results in [0, MAX_RESULTS_CEILING + 1] {
        let (status, response) = emulator
            .call("ListChannels", &json!({ "MaxResults": max_results }))
            .await;
        assert_eq!(
            status, 400,
            "case max-results-{max_results}: a MaxResults outside the modelled range is \
             refused, got {status} with body {response}"
        );
    }
}

/// One row of the filter table: the `StreamFilter` a request carries, and the channels the rule
/// keeps for it.
struct FilterRow {
    case: &'static str,
    filter: Option<Value>,
    kept: Vec<&'static str>,
}

// 3. A `StreamFilter` keeps the channels sourcing the streams it names
#[tokio::test]
async fn a_stream_filter_keeps_the_channels_sourcing_those_streams() {
    let emulator = Emulator::new();
    let one = on_demand_stream(&emulator, "filter-source-one").await;
    let two = on_demand_stream(&emulator, "filter-source-two").await;
    let on_one = s3_channel(&emulator, REGION, "filter-a", &one).await;
    s3_channel(&emulator, REGION, "filter-b", &two).await;
    s3_channel(&emulator, REGION, "filter-c", &one).await;

    // The creation instant of the stream `filter-a` sources, as the stored record reports it.
    let created_at = on_one
        .get("StreamConfigurationList")
        .and_then(Value::as_array)
        .and_then(|streams| streams.first())
        .and_then(|stream| stream.get("StreamCreationTimestamp"))
        .cloned()
        .unwrap_or_else(|| panic!("the create must report its source stream's creation instant"));

    // The rows differ along how many streams the filter names, whether it pins their creation
    // instants, and whether the streams it names are ones a channel sources at all.
    let rows = vec![
        FilterRow {
            case: "no-filter-at-all",
            filter: None,
            kept: vec!["filter-a", "filter-b", "filter-c"],
        },
        FilterRow {
            case: "one-stream-two-channels",
            filter: Some(json!([{ "StreamARN": one }])),
            kept: vec!["filter-a", "filter-c"],
        },
        FilterRow {
            case: "one-stream-one-channel",
            filter: Some(json!([{ "StreamARN": two }])),
            kept: vec!["filter-b"],
        },
        FilterRow {
            case: "both-streams",
            filter: Some(json!([{ "StreamARN": one }, { "StreamARN": two }])),
            kept: vec!["filter-a", "filter-b", "filter-c"],
        },
        FilterRow {
            case: "the-stream-this-incarnation-was-created-at",
            filter: Some(json!([{
                "StreamARN": one,
                "StreamCreationTimestamp": created_at,
            }])),
            kept: vec!["filter-a", "filter-c"],
        },
        FilterRow {
            // An ARN paired with an instant that is not this stream's names another
            // incarnation of the name, which no channel of this region sources.
            case: "another-incarnation-of-the-stream",
            filter: Some(json!([{ "StreamARN": one, "StreamCreationTimestamp": 1.0_f64 }])),
            kept: vec![],
        },
        FilterRow {
            case: "a-stream-no-channel-sources",
            filter: Some(json!([{
                "StreamARN": format!("{one}-elsewhere"),
            }])),
            kept: vec![],
        },
    ];

    for row in &rows {
        let mut body = json!({});
        if let Some(filter) = &row.filter {
            body["StreamFilter"] = filter.clone();
        }
        let listed = emulator.ok(row.case, "ListChannels", &body).await;
        assert_eq!(
            listed_names(row.case, &listed),
            row.kept,
            "case {}: expected the channels sourcing the filtered streams, got {listed}",
            row.case
        );

        // The filter holds page by page: a paginating client resends it, and every page is cut
        // from the same filtered listing.
        let mut paged_body = body.clone();
        paged_body["MaxResults"] = json!(1);
        let paged = every_page(&emulator, row.case, &paged_body, 1).await;
        assert_eq!(
            paged, row.kept,
            "case {}: expected the filter to hold on every page, got {paged:?}",
            row.case
        );
    }
}

// 4. A listing reports the channels of the region the request was signed for
#[tokio::test]
async fn a_listing_is_scoped_to_the_region_it_was_signed_for() {
    let emulator = Emulator::new();
    let elsewhere = "us-west-2";
    let shared_name = "regional-channel";
    let here = on_demand_stream(&emulator, "regional-source").await;
    let there = on_demand_stream_in(&emulator, elsewhere, "regional-source").await;
    s3_channel(&emulator, REGION, shared_name, &here).await;
    s3_channel(&emulator, REGION, "only-here", &here).await;
    s3_channel(&emulator, elsewhere, shared_name, &there).await;

    let case = "regional";
    let listed = emulator.ok(case, "ListChannels", &json!({})).await;
    assert_eq!(
        listed_names(case, &listed),
        vec!["only-here", shared_name],
        "case {case}: expected only the channels of {REGION}, got {listed}"
    );
    assert_eq!(
        member(case, &summary_of(case, &listed, shared_name), "ChannelARN"),
        channel_arn(REGION, shared_name),
        "case {case}: expected the ARN of the channel created in {REGION}, got {listed}"
    );

    let there_listed = emulator.ok_in(case, elsewhere, "ListChannels", &json!({})).await;
    assert_eq!(
        listed_names(case, &there_listed),
        vec![shared_name],
        "case {case}: expected only the channels of {elsewhere}, got {there_listed}"
    );
    assert_eq!(
        member(case, &summary_of(case, &there_listed, shared_name), "ChannelARN"),
        channel_arn(elsewhere, shared_name),
        "case {case}: expected the ARN of the channel created in {elsewhere}, got \
         {there_listed}"
    );
}

/// One row of the refusal table: the `NextToken` a request carries, and the modelled error the
/// rule answers it with.
struct RefusalRow {
    case: &'static str,
    token: String,
    /// The modelled error code the reason this cursor names no page belongs to.
    expected_code: &'static str,
    /// The sentence that error class is worded with.
    expected_message: String,
}

// 5. A cursor this listing never issued, or one past its lifetime, names no page
#[tokio::test]
async fn a_listing_refuses_a_cursor_it_never_issued() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "cursor-source").await;
    for name in ["cursor-a", "cursor-b"] {
        s3_channel(&emulator, REGION, name, &source).await;
    }

    let rows = vec![
        RefusalRow {
            case: "a NextToken this service never issued",
            token: "not-a-token".to_string(),
            expected_code: "InvalidArgumentException",
            expected_message: INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        },
        RefusalRow {
            case: "a NextToken past its documented lifetime",
            token: RegionalPageToken::new(
                RegionalListing::Channels,
                REGION,
                "cursor-b",
                SystemTime::now() - PAGE_TOKEN_TTL - Duration::from_secs(1),
            )
            .encode(),
            expected_code: "ExpiredNextTokenException",
            expected_message: next_token_expired_message(),
        },
        RefusalRow {
            case: "a NextToken minted at an instant that has not arrived",
            token: RegionalPageToken::new(
                RegionalListing::Channels,
                REGION,
                "cursor-b",
                SystemTime::now() + Duration::from_secs(3_600),
            )
            .encode(),
            expected_code: "ExpiredNextTokenException",
            expected_message: next_token_expired_message(),
        },
        RefusalRow {
            case: "a NextToken minted for another region",
            token: RegionalPageToken::new(
                RegionalListing::Channels,
                "us-west-2",
                "cursor-b",
                SystemTime::now(),
            )
            .encode(),
            expected_code: "InvalidArgumentException",
            expected_message: INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        },
        RefusalRow {
            // A cursor of ANOTHER listing of this service names no page of this one, even
            // minted for this region at this instant.
            case: "a NextToken minted for another listing",
            token: RegionalPageToken::new(
                RegionalListing::Streams,
                REGION,
                "cursor-b",
                SystemTime::now(),
            )
            .encode(),
            expected_code: "InvalidArgumentException",
            expected_message: INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        },
    ];

    for row in &rows {
        let (status, response) = emulator
            .call("ListChannels", &json!({ "NextToken": row.token }))
            .await;
        assert_eq!(
            status, 400,
            "case {}: expected a cursor that names no page to be refused, got {status} with \
             body {response}",
            row.case
        );
        assert_eq!(
            error_code(&response),
            row.expected_code,
            "case {}: expected {}, got {response}",
            row.case,
            row.expected_code
        );
        assert_eq!(
            error_message(&response),
            row.expected_message,
            "case {}: expected the published refusal wording {:?}, got {response}",
            row.case,
            row.expected_message
        );
    }

    // A cursor this region issued is still served, so the refusals above turn on the cursor and
    // not on the member.
    let case = "a live NextToken";
    let live =
        RegionalPageToken::new(RegionalListing::Channels, REGION, "cursor-b", SystemTime::now())
            .encode();
    let resumed = emulator
        .ok(case, "ListChannels", &json!({ "NextToken": live }))
        .await;
    assert_eq!(
        listed_names(case, &resumed),
        vec!["cursor-b"],
        "case {case}: expected the cursor to resume at the channel it names, got {resumed}"
    );
}

/// A channel the listing reports keeps the bucket the create named, which is what tells the
/// stored record apart from one built out of the request.
#[tokio::test]
async fn a_summary_comes_from_the_stored_record() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "stored-source").await;
    let case = "stored";
    s3_channel(&emulator, REGION, "stored-channel", &source).await;

    let described = emulator
        .ok(
            case,
            "DescribeChannel",
            &json!({ "ChannelARN": channel_arn(REGION, "stored-channel") }),
        )
        .await;
    let described = description(case, &described);
    assert_eq!(
        described
            .get("S3DestinationConfiguration")
            .and_then(|destination| destination.get("StorageConfiguration"))
            .and_then(|storage| storage.get("BucketARN"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        bucket_arn("bucket-stored-channel"),
        "case {case}: expected the stored destination bucket, got {described}"
    );

    let listed = emulator.ok(case, "ListChannels", &json!({})).await;
    let summary = summary_of(case, &listed, "stored-channel");
    assert_eq!(
        summary.get("ChannelId"),
        described.get("ChannelId"),
        "case {case}: expected the listing to report the stored channel's id, got {summary} \
         beside {described}"
    );
    assert_eq!(
        member(case, &summary, "ChannelDestinationType"),
        DESTINATION_S3,
        "case {case}: expected the destination kind the stored record carries, got {summary}"
    );
}
