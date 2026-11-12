// SPDX-License-Identifier: CC0-1.0

//! How EVERY `NextToken`-taking Kinesis listing classifies a cursor it will not accept, end
//! to end over the wire.
//!
//! RULE: a pagination token is refused under the error class the REASON it names no page
//! belongs to. A string that is not a cursor this listing issued — its bytes do not decode,
//! the seal they carry does not verify them, or it was minted for another listing or another
//! region — is a parameter that "can't be used": HTTP 400, `InvalidArgumentException`, with
//! the fixed sentence "Invalid NextToken.". `ExpiredNextTokenException` is reserved for a
//! cursor the listing DID issue whose lifetime has elapsed ("The pagination token passed to
//! the operation is expired"). Verification is decided BEFORE the lifetime, so the two
//! refusals stay disjoint — a paginating client restarts its scan on the expiry and fails
//! fast on the invalid argument, so collapsing them loops the client forever.
//!
//! The recorded flow this was derived from takes one `ListStreamConsumers` cursor and
//! replaces its last eight characters, which is one row of the table below. The rows differ
//! along what the rule generalises over: WHICH listing issued the cursor (`ListStreams`,
//! `ListShards`, `ListStreamConsumers`, `ListChannels` — the four operations the model binds
//! a `NextToken` and both error shapes to), and HOW the cursor was altered (its tail, its
//! head or its middle rewritten, a byte dropped or appended, an odd character count, a
//! character no hex alphabet carries, the payload with the service's seal stripped off, and
//! text the service never encoded at all).
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    next_token_expired_message, shard_id, ConsumerPageToken, RegionalListing, RegionalPageToken,
    ShardPageToken, INVALID_NEXT_TOKEN_MESSAGE, PAGE_TOKEN_TTL,
};

use crate::stream_tags::{error_code, error_message, past_activation, Emulator, REGION};

/// One way a cursor can reach the service altered, and the name the failure reports it by.
struct Mangling {
    case: &'static str,
    alter: fn(&str) -> String,
}

/// The recorded instance: the last eight characters replaced, as a token comes back from a
/// lossy round trip through a queue or a store.
fn tail_rewritten(token: &str) -> String {
    format!("{}deadbeef", &token[..token.len() - 8])
}

/// The same alteration at the other end of the cursor.
fn head_rewritten(token: &str) -> String {
    format!("deadbeef{}", &token[8..])
}

/// ONE character changed, in the middle, for another of the same alphabet: the cursor still
/// decodes to bytes, and only those bytes differ.
fn middle_flipped(token: &str) -> String {
    let middle = token.len() / 2;
    let mut altered: Vec<char> = token.chars().collect();
    altered[middle] = if altered[middle] == '0' { '1' } else { '0' };
    altered.into_iter().collect()
}

/// A cursor cut short by two characters — one byte of it lost.
fn byte_dropped(token: &str) -> String {
    token[..token.len() - 2].to_string()
}

/// A cursor with one byte of padding appended.
fn byte_appended(token: &str) -> String {
    format!("{token}00")
}

/// A cursor cut short by a single character, so what it carries is not a whole number of
/// bytes.
fn half_byte_dropped(token: &str) -> String {
    token[..token.len() - 1].to_string()
}

/// A cursor carrying a character the hex alphabet does not.
fn out_of_alphabet(token: &str) -> String {
    format!("z{}", &token[1..])
}

/// The cursor's own payload with the eight-byte seal the service appends stripped off: a
/// cursor a caller built for itself out of a cursor it was handed, which is exactly what the
/// seal exists to refuse.
fn seal_stripped(token: &str) -> String {
    token[..token.len() - 16].to_string()
}

/// A string the service never encoded at all.
fn never_issued(_token: &str) -> String {
    "not-a-token-this-service-issued".to_string()
}

/// Every alteration the rule covers, each applied to every listing's own cursor.
const MANGLINGS: &[Mangling] = &[
    Mangling {
        case: "the last eight characters replaced",
        alter: tail_rewritten,
    },
    Mangling {
        case: "the first eight characters replaced",
        alter: head_rewritten,
    },
    Mangling {
        case: "one character flipped in the middle",
        alter: middle_flipped,
    },
    Mangling {
        case: "a byte dropped from the end",
        alter: byte_dropped,
    },
    Mangling {
        case: "a byte appended",
        alter: byte_appended,
    },
    Mangling {
        case: "an odd number of characters",
        alter: half_byte_dropped,
    },
    Mangling {
        case: "a character outside the hex alphabet",
        alter: out_of_alphabet,
    },
    Mangling {
        case: "the payload with its seal stripped off",
        alter: seal_stripped,
    },
    Mangling {
        case: "text the service never encoded",
        alter: never_issued,
    },
];

/// One listing that hands out a `NextToken`: how a page of it is asked for, a cursor it
/// issued that is still inside its lifetime, and one of its own cursors that is past it.
struct Listing {
    operation: &'static str,
    /// The members a resumed request of this listing carries beside the `NextToken`.
    base: Value,
    /// The response member a served page of this listing carries.
    items_member: &'static str,
    /// A cursor this listing issued, inside its lifetime.
    live: String,
    /// A cursor this listing issued whose lifetime has elapsed.
    expired: String,
}

/// A request body for `listing` resuming at `token`.
fn resumed(listing: &Listing, token: &str) -> Value {
    let mut body = listing.base.clone();
    body.as_object_mut()
        .expect("a request body is a JSON object")
        .insert("NextToken".to_string(), json!(token));
    body
}

/// An instant far enough in the past that a cursor minted at it is outside the lifetime a
/// cursor of this service carries.
fn past_lifetime() -> SystemTime {
    SystemTime::now() - PAGE_TOKEN_TTL - Duration::from_secs(1)
}

/// Create the streams and consumers the four listings page, then take one live cursor from
/// each listing over the wire and mint one expired cursor of each.
async fn listings(emulator: &Emulator) -> Vec<Listing> {
    let case = "setup";
    // Every create is armed first and waited out ONCE, so the setup costs one transition.
    for (name, shards) in [("cursor-alpha", 1), ("cursor-beta", 1), ("cursor-shards", 2)] {
        emulator
            .ok(
                case,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": shards }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    let consumer_stream = emulator
        .ok(
            case,
            "DescribeStreamSummary",
            &json!({ "StreamName": "cursor-alpha" }),
        )
        .await["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: DescribeStreamSummary reported no StreamARN"))
        .to_string();
    for consumer in ["cursor-consumer-a", "cursor-consumer-b"] {
        emulator
            .ok(
                case,
                "RegisterStreamConsumer",
                &json!({ "StreamARN": consumer_stream, "ConsumerName": consumer }),
            )
            .await;
    }

    let shard_stream = emulator
        .ok(
            case,
            "DescribeStreamSummary",
            &json!({ "StreamName": "cursor-shards" }),
        )
        .await["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: DescribeStreamSummary reported no StreamARN"))
        .to_string();

    let (consumer_created_at, shard_created_at) = {
        let state = emulator.store();
        let store = state.lock();
        let now = SystemTime::now();
        (
            store
                .stream_in(now, REGION, "cursor-alpha")
                .unwrap_or_else(|| panic!("case {case}: the store holds no stream cursor-alpha"))
                .created_at,
            store
                .stream_in(now, REGION, "cursor-shards")
                .unwrap_or_else(|| panic!("case {case}: the store holds no stream cursor-shards"))
                .created_at,
        )
    };

    let listings = vec![
        Listing {
            operation: "ListStreams",
            base: json!({}),
            items_member: "StreamNames",
            live: first_page_token(emulator, "ListStreams", &json!({ "Limit": 1 })).await,
            expired: RegionalPageToken::new(
                RegionalListing::Streams,
                REGION,
                "cursor-beta",
                past_lifetime(),
            )
            .encode(),
        },
        Listing {
            operation: "ListShards",
            base: json!({}),
            items_member: "Shards",
            live: first_page_token(
                emulator,
                "ListShards",
                &json!({ "StreamName": "cursor-shards", "MaxResults": 1 }),
            )
            .await,
            expired: ShardPageToken::new(
                &shard_stream,
                shard_created_at,
                &shard_id(1),
                past_lifetime(),
            )
            .encode(),
        },
        Listing {
            operation: "ListStreamConsumers",
            base: json!({ "StreamARN": consumer_stream }),
            items_member: "Consumers",
            live: first_page_token(
                emulator,
                "ListStreamConsumers",
                &json!({ "StreamARN": consumer_stream, "MaxResults": 1 }),
            )
            .await,
            expired: ConsumerPageToken::new(
                &consumer_stream,
                consumer_created_at,
                "cursor-consumer-b",
                past_lifetime(),
            )
            .encode(),
        },
        Listing {
            operation: "ListChannels",
            base: json!({}),
            items_member: "ChannelSummaries",
            // This listing pages the region's channels, which no create of the setup above
            // adds: a cursor it issued is minted here instead, and a cursor naming a channel
            // the region does not hold still serves a page — an empty one.
            live: RegionalPageToken::new(
                RegionalListing::Channels,
                REGION,
                "cursor-channel",
                SystemTime::now(),
            )
            .encode(),
            expired: RegionalPageToken::new(
                RegionalListing::Channels,
                REGION,
                "cursor-channel",
                past_lifetime(),
            )
            .encode(),
        },
    ];

    for listing in &listings {
        assert!(
            listing.live.len() > 32,
            "case {case}: {} must hand back a cursor long enough for every alteration of \
             the table, got {:?}",
            listing.operation,
            listing.live
        );
    }
    listings
}

/// The `NextToken` the first, truncated page of `operation` hands back.
async fn first_page_token(emulator: &Emulator, operation: &str, body: &Value) -> String {
    let page = emulator.ok("setup", operation, body).await;
    page["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case setup: a truncated {operation} page must hand back a NextToken, got {page}")
        })
        .to_string()
}

/// Assert `listing` serves a page for the cursor `token`, which is what makes a refusal of an
/// altered copy of it a judgement on the alteration and nothing else.
async fn assert_serves(emulator: &Emulator, case: &str, listing: &Listing) {
    let page = emulator
        .ok(case, listing.operation, &resumed(listing, &listing.live))
        .await;
    assert!(
        page.get(listing.items_member).is_some(),
        "case {case}: a page of {} carries {}, got {page}",
        listing.operation,
        listing.items_member
    );
}

// 1. A cursor that does not decode and verify is an INVALID ARGUMENT, never an expiry
#[tokio::test]
async fn an_altered_next_token_is_refused_as_an_invalid_argument() {
    let emulator = Emulator::new();
    let listings = listings(&emulator).await;

    for listing in &listings {
        // The cursor works, immediately before every alteration of it is refused.
        assert_serves(&emulator, "a live cursor before the alterations", listing).await;

        for mangling in MANGLINGS {
            let case = format!("{} with {}", listing.operation, mangling.case);
            let altered = (mangling.alter)(&listing.live);
            assert_ne!(
                altered, listing.live,
                "case {case}: the row must alter the cursor, got the cursor itself"
            );

            let (status, response) = emulator
                .call(listing.operation, &resumed(listing, &altered))
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected a 400, got {status} with body {response}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidArgumentException",
                "case {case}: expected InvalidArgumentException for a cursor that does not \
                 decode and verify, got {response}"
            );
            assert_eq!(
                error_message(&response),
                INVALID_NEXT_TOKEN_MESSAGE,
                "case {case}: expected the message {INVALID_NEXT_TOKEN_MESSAGE:?}, got \
                 {response}"
            );
        }

        // ... and still works after them, so nothing about the refusals was an expiry and no
        // refusal consumed the cursor.
        assert_serves(&emulator, "a live cursor after the alterations", listing).await;
    }
}

// 2. A cursor the listing DID issue is the expiry, and only once its lifetime has elapsed
#[tokio::test]
async fn a_cursor_this_listing_issued_expires_only_past_its_lifetime() {
    let emulator = Emulator::new();
    let listings = listings(&emulator).await;

    for listing in &listings {
        let case = format!("{} inside its cursor's lifetime", listing.operation);
        assert_serves(&emulator, &case, listing).await;

        let case = format!("{} past its cursor's lifetime", listing.operation);
        let (status, response) = emulator
            .call(listing.operation, &resumed(listing, &listing.expired))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ExpiredNextTokenException",
            "case {case}: expected ExpiredNextTokenException for a cursor this listing \
             issued whose lifetime has elapsed, got {response}"
        );
        assert_eq!(
            error_message(&response),
            next_token_expired_message(),
            "case {case}: expected the published expiry wording, got {response}"
        );
    }
}
