// SPDX-License-Identifier: CC0-1.0

//! The FRAMING of this service's answers on the wire, end to end over HTTP.
//!
//! The harness is the one `tagging_wire_form.rs` owns, so every assertion here goes through
//! the router: a request is built, dispatched to the mounted service, and the status, the
//! headers and the body read back.
//!
//! RULE: a success at the no-content status (HTTP 204) carries no body and states the
//! length it has — `content-length: 0` — while every answer that CARRIES content keeps the
//! body and the headers its renderer wrote. Both of this service's tagging writes model
//! `smithy.api#Unit` as their output and 204 as their success code, as the `TagResource` and
//! `UntagResource` entries in this crate's `smithy-model/` document state, so the rows below
//! span both of them, and the readers span the other half of the rule.

use serde_json::json;

use crate::tagging_wire_form::{encode, Emulator, ACCOUNT, REGION};

/// The status a success that carries no content answers, and the length it states for the
/// body it does not carry.
const NO_CONTENT_STATUS: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// A channel to write tags onto, with one tag of its own.
async fn channel(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .request(
            "POST",
            &format!("/channel/{}", encode(name)),
            Some(json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "tags": { "Env": "prod" },
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the channel the rule is read off, got {} {}",
        created.status, created.error_code
    );
    format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:channel/{name}")
}

/// Every success at the no-content status carries no body and states the length it has.
#[tokio::test]
async fn a_no_content_answer_states_its_length() {
    let emulator = Emulator::new();
    let arn = channel(&emulator, "framing-probe").await;

    // Both operations whose modelled output is `smithy.api#Unit` at HTTP 204, each in a
    // request that really does the work: the write, then the removal of what it wrote.
    let tagged = emulator
        .request(
            "POST",
            &format!("/tags/{}", encode(&arn)),
            Some(json!({ "tags": { "Team": "video" } })),
        )
        .await;
    let untagged = emulator.untag(&arn, &["Team"]).await;

    for (case, reply) in [("TagResource", tagged), ("UntagResource", untagged)] {
        assert_eq!(
            reply.status, NO_CONTENT_STATUS,
            "case {case}: expected the modelled no-content status, got {} {}",
            reply.status, reply.error_code
        );
        assert!(
            reply.body_bytes.is_empty(),
            "case {case}: a no-content answer carries no body, got {}",
            String::from_utf8_lossy(&reply.body_bytes)
        );
        assert_eq!(
            reply.content_length.as_deref(),
            Some(NO_CONTENT_LENGTH),
            "case {case}: expected the length of the body it does not carry, got {:?}",
            reply.content_length
        );
    }

    // The removal really happened: the framing correction frames an answer, it does not
    // short-circuit the work behind it.
    assert_eq!(
        emulator.list_tags(&arn).await.get("/tags"),
        &json!({ "Env": "prod" }),
        "the untag removed the key it named and left the others"
    );
}

/// An answer that CARRIES content is untouched by the framing rule: the body is the
/// renderer's, member for member.
#[tokio::test]
async fn an_answer_that_carries_content_keeps_its_body() {
    let emulator = Emulator::new();
    let name = "content-probe";
    let arn = channel(&emulator, name).await;

    // (case, the answer, the member it reports)
    let created = emulator
        .request("GET", &format!("/channel/{}", encode(name)), None)
        .await;
    let listed = emulator.list_tags(&arn).await;
    for (case, reply, pointer, expected) in [
        ("DescribeChannel", created, "/ChannelName", json!(name)),
        (
            "ListTagsForResource",
            listed,
            "/tags",
            json!({ "Env": "prod" }),
        ),
    ] {
        assert_eq!(
            reply.status, 200,
            "case {case}: expected the read to succeed, got {} {}",
            reply.status, reply.error_code
        );
        assert!(
            !reply.body_bytes.is_empty(),
            "case {case}: an answer that carries content keeps its body"
        );
        assert_eq!(
            reply.get(pointer),
            &expected,
            "case {case}: the body is the renderer's, got {}",
            reply.body
        );
    }
}
