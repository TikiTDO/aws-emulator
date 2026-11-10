// SPDX-License-Identifier: CC0-1.0

//! How the ARN-addressed tagging operations report a REFUSED call, end to end over the
//! wire.
//!
//! `TagResource`, `UntagResource` and `ListTagsForResource` address a pipe by its
//! `PipeArn` and are served by the resource-tagging layer that fronts this API, which is
//! not the pipes control plane. Two consequences, both tested here over tables rather
//! than one request:
//!
//! * the layer resolves no pipe NAME, so an ARN it cannot resolve is reported by ONE
//!   constant, resource-agnostic sentence — not by the name-interpolated sentence the
//!   name-addressed control-plane operations (`DescribePipe`/`StartPipe`/`StopPipe`/
//!   `DeletePipe`/`UpdatePipe`) report, which `tests/all/pipe_refusal_bodies.rs` pins;
//! * the layer refuses a tag whose content breaks a published tag rule with its own error
//!   code, `BadRequestException`, which no operation of this model declares — including
//!   a key named for REMOVAL that is outside `TagKey`'s `@length(1, 128)` bound, where
//!   the refusal states the member `UntagResource`'s request validation names and both
//!   published bounds — and refuses an ARN of ANOTHER vendor with the same code, because no
//!   pipe of this service can ever carry it.
//!
//! What a tag may CONTAIN has its own suite in `tests/all/tag_key_rules.rs`; the merge
//! semantics live in `tests/all/pipe_tagging.rs`. Every assertion goes through
//! `MultiServiceRouter::dispatch`, reusing those suites' helpers rather than copying
//! them, and every refused call is checked to have persisted nothing.

use std::collections::BTreeMap;

use serde_json::json;

use crate::pipe_lifecycle::{call, create, describe, router, REGION};
use crate::pipe_tagging::{
    assert_tags_agree, create_body, list_tags, pipe_arn, tag, tag_map, untag,
    TAG_MUTATION_STATUS,
};
use crate::tagging_refusal_wording::tag_refusal_document;

/// The sentence the ARN-addressed tagging operations report an unresolvable `resourceArn`
/// with. Recorded behaviour: a constant, not built from the request.
const RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The sentence those operations report an ARN of another vendor with. The vendor it names
/// is this service's own ARN namespace.
const NOT_A_PIPE_ARN: &str = "Invalid input resource arn: expected vendor pipes";

/// The code the tagging layer refuses a broken tag rule with.
const TAG_RULE_CODE: &str = "BadRequestException";

/// `TagKey`'s published bounds. Source: the `TagKey` shape of
/// `smithy-model/com-amazonaws-pipes-pipes.json`.
const MAX_TAG_KEY_LEN: usize = 128;
const MIN_TAG_KEY_LEN: usize = 1;

/// The most BYTES of a refused value a validation sentence echoes before eliding it.
const MAX_ECHOED_BYTES: usize = 256;

/// RULE: whichever of the three ARN-addressed tagging operations is called, an ARN of THIS
/// vendor that this account holds no pipe for is 404 `NotFoundException` carrying the one
/// constant sentence — nothing of the ARN interpolated into it.
///
/// The rows span the ways such an ARN resolves to no pipe: never created, created and then
/// deleted, another region, another account, a `pipes` resource that is not a pipe at all,
/// and one whose pipe name is empty. (An ARN of ANOTHER vendor is a different refusal — see
/// `an_arn_of_another_vendor_is_refused_as_a_bad_request`.)
#[tokio::test]
async fn an_unresolvable_resource_arn_is_reported_by_one_constant_sentence() {
    let router = router();

    // One live pipe, so the 404s below are about the ARN and not an empty store.
    let live = "arnnotfoundlive";
    let created = create(&router, live, create_body(Some(&json!({"env": "prod"})))).await;
    assert_eq!(
        created.status, 200,
        "the live pipe must be created, got {created:?}"
    );

    // A pipe that existed and was deleted: its ARN stops naming anything.
    let deleted = "arnnotfounddeleted";
    let created = create(&router, deleted, create_body(Some(&json!({"env": "prod"})))).await;
    assert_eq!(
        created.status, 200,
        "the pipe to delete must be created, got {created:?}"
    );
    let removed = call(
        &router,
        "DELETE",
        &format!("/v1/pipes/{deleted}"),
        None,
        REGION,
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "DeletePipe must succeed, got {removed:?}"
    );

    let account = engine::EMULATOR_ACCOUNT_ID;
    let rows: Vec<(&str, String)> = vec![
        ("never-created", pipe_arn("arnnotfoundghost")),
        ("deleted-pipe", pipe_arn(deleted)),
        (
            "another-region",
            format!("arn:aws:pipes:eu-central-1:{account}:pipe/{live}"),
        ),
        (
            "another-account",
            format!("arn:aws:pipes:{REGION}:210987654321:pipe/{live}"),
        ),
        (
            "a-pipes-resource-that-is-not-a-pipe",
            format!("arn:aws:pipes:{REGION}:{account}:{live}"),
        ),
        (
            "a-resource-type-that-only-looks-like-a-pipe",
            format!("arn:aws:pipes:{REGION}:{account}:pipeline/{live}"),
        ),
        (
            "an-empty-pipe-name",
            format!("arn:aws:pipes:{REGION}:{account}:pipe/"),
        ),
    ];

    for (case, resource_arn) in rows {
        let name_in_arn = resource_arn.rsplit('/').next().unwrap_or_default();
        for (operation, reply) in [
            ("ListTagsForResource", list_tags(&router, &resource_arn).await),
            (
                "TagResource",
                tag(&router, &resource_arn, &json!({"env": "prod"})).await,
            ),
            ("UntagResource", untag(&router, &resource_arn, &["env"]).await),
        ] {
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case:?} / {operation}: expected 404 NotFoundException, got {reply:?}"
            );
            assert_eq!(
                reply.message(),
                RESOURCE_NOT_FOUND,
                "case {case:?} / {operation}: expected the one constant sentence, \
                 got {reply:?}"
            );
            // The constant sentence resolves nothing and echoes nothing: neither the pipe
            // the ARN names nor any other part of it reaches the caller.
            assert!(
                !reply.message().contains("arn:")
                    && !reply.message().contains(account)
                    && !reply.message().contains(REGION)
                    && (name_in_arn.is_empty() || !reply.message().contains(name_in_arn)),
                "case {case:?} / {operation}: the sentence must echo no part of the ARN it \
                 was given, got {reply:?}"
            );
        }
    }

    // The refused calls wrote nothing: the live pipe still carries what it was created
    // with, and the deleted one is still gone.
    assert_tags_agree(
        &router,
        "live-pipe-after-the-404s",
        live,
        &BTreeMap::from([("env".to_string(), "prod".to_string())]),
    )
    .await;
    let described = describe(&router, deleted).await;
    assert_eq!(
        described.status, 404,
        "the deleted pipe must stay deleted, got {described:?}"
    );
}

/// RULE: an ARN of ANOTHER vendor can name no pipe of this service however this account's
/// pipes change, so the tagging layer refuses it as a bad request: 400 `BadRequestException`
/// with the one sentence naming the vendor the layer expected, echoing no part of the ARN it
/// was handed, on all three operations and leaving every stored tag alone.
///
/// The rows span the ways an ARN names another vendor: another service's own resource, one
/// whose resource part looks exactly like a pipe's, and one whose vendor is close to this
/// service's name.
#[tokio::test]
async fn an_arn_of_another_vendor_is_refused_as_a_bad_request() {
    let router = router();
    let live = "arnnotapipelive";
    let stored = json!({"env": "prod"});
    let created = create(&router, live, create_body(Some(&stored))).await;
    assert_eq!(
        created.status, 200,
        "the live pipe must be created, got {created:?}"
    );

    let account = engine::EMULATOR_ACCOUNT_ID;
    let rows: Vec<(&str, String)> = vec![
        (
            "another-service-resource",
            format!("arn:aws:sqs:{REGION}:{account}:orders"),
        ),
        (
            "another-service-with-a-pipe-resource",
            format!("arn:aws:sqs:{REGION}:{account}:pipe/{live}"),
        ),
        (
            "a-vendor-close-to-this-one",
            format!("arn:aws:pipe:{REGION}:{account}:pipe/{live}"),
        ),
    ];

    for (case, resource_arn) in rows {
        for (operation, reply) in [
            ("ListTagsForResource", list_tags(&router, &resource_arn).await),
            (
                "TagResource",
                tag(&router, &resource_arn, &json!({"env": "prod"})).await,
            ),
            ("UntagResource", untag(&router, &resource_arn, &["env"]).await),
        ] {
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (400, TAG_RULE_CODE),
                "case {case:?} / {operation}: expected 400 {TAG_RULE_CODE}, got {reply:?}"
            );
            assert_eq!(
                reply.message(),
                NOT_A_PIPE_ARN,
                "case {case:?} / {operation}: expected the one fixed sentence, got {reply:?}"
            );
            assert!(
                !reply.message().contains(account)
                    && !reply.message().contains(REGION)
                    && !reply.message().contains(live),
                "case {case:?} / {operation}: the sentence must echo no part of the ARN it \
                 was given, got {reply:?}"
            );
        }
    }

    // None of the refused calls touched the pipe those ARNs came close to naming.
    assert_tags_agree(&router, "live-pipe-after-the-400s", live, &tag_map(&stored)).await;
}

/// RULE: the same pipe answered through its NAME keeps the name-interpolated sentence —
/// the two addressing forms are reported differently, and that is what distinguishes
/// them. Asserted side by side on ONE absent pipe, so neither sentence can drift into
/// the other.
#[tokio::test]
async fn the_two_addressing_forms_report_an_absent_pipe_differently() {
    let router = router();
    let absent = "arnformsghost";

    let described = describe(&router, absent).await;
    assert_eq!(
        (described.status, described.error_code.as_str()),
        (404, "NotFoundException"),
        "DescribePipe on an absent pipe must be a 404, got {described:?}"
    );
    assert_eq!(
        described.message(),
        format!("Pipe {absent} does not exist."),
        "the name-addressed form must name the pipe the request asked for, got {described:?}"
    );

    let listed = list_tags(&router, &pipe_arn(absent)).await;
    assert_eq!(
        (listed.status, listed.error_code.as_str()),
        (404, "NotFoundException"),
        "ListTagsForResource on the same absent pipe must be a 404, got {listed:?}"
    );
    assert_eq!(
        listed.message(),
        RESOURCE_NOT_FOUND,
        "the ARN-addressed form must state the constant sentence, got {listed:?}"
    );
    assert_ne!(
        listed.message(),
        described.message(),
        "the two addressing forms must not report the same sentence, got {listed:?}"
    );
}

/// RULE: a tag key named for REMOVAL that is outside `TagKey`'s published bound is
/// refused with 400 `BadRequestException` stating the member `UntagResource`'s request
/// validation names, the key list it was sent in, and both published bounds — inside the
/// JSON document that layer's message is (`tests/all/tagging_refusal_wording.rs`) — and
/// the pipe's tags are left exactly as they were.
///
/// The rows sit at the bound and past it, from both ends, with the offending key alone
/// and beside a key the pipe does carry, and over a unicode key (the bound counts
/// characters, not bytes).
#[tokio::test]
async fn untag_resource_refuses_a_key_outside_the_published_bound() {
    let router = router();
    let name = "untagkeybound";
    let stored = json!({"env": "prod", "team": "core"});
    let created = create(&router, name, create_body(Some(&stored))).await;
    assert_eq!(
        created.status, 200,
        "CreatePipe must succeed, got {created:?}"
    );
    let arn = pipe_arn(name);

    let at_bound = "k".repeat(MAX_TAG_KEY_LEN);
    let past_bound = "k".repeat(MAX_TAG_KEY_LEN + 1);
    // 129 characters, 258 bytes: the bound counts CHARACTERS, so this is past it. Past
    // the 256 BYTES a refusal may echo, too, so the sentence states a prefix cut on a
    // character boundary and the full byte count — a request cannot be amplified into an
    // arbitrarily long response.
    let unicode_past_bound = "é".repeat(MAX_TAG_KEY_LEN + 1);
    let unicode_echo = format!(
        "{}... [{} bytes]",
        "é".repeat(MAX_ECHOED_BYTES / "é".len()),
        unicode_past_bound.len(),
    );

    // (the case, the keys the call names, the key as the refusal must echo it — `None`
    // when the call is accepted).
    let rows: Vec<(&str, Vec<String>, Option<String>)> = vec![
        ("a-key-at-the-bound-is-accepted", vec![at_bound], None),
        (
            "one-character-past-the-bound",
            vec![past_bound.clone()],
            Some(past_bound.clone()),
        ),
        (
            "past-the-bound-beside-a-stored-key",
            vec!["team".to_string(), past_bound.clone()],
            Some(past_bound.clone()),
        ),
        (
            "a-unicode-key-past-the-bound",
            vec![unicode_past_bound],
            Some(unicode_echo),
        ),
    ];

    for (case, keys, echoed_key) in rows {
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
        let reply = untag(&router, &arn, &borrowed).await;
        match &echoed_key {
            None => assert_eq!(
                reply.status, TAG_MUTATION_STATUS,
                "case {case:?}: a key inside the bound must be accepted, got {reply:?}"
            ),
            Some(key) => {
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (400, TAG_RULE_CODE),
                    "case {case:?}: expected 400 {TAG_RULE_CODE}, got {reply:?}"
                );
                assert_eq!(
                    reply.message(),
                    tag_refusal_document(&format!(
                        "1 validation error detected: Value '[{key}]' at 'removedTagKeys' \
                         failed to satisfy constraint: Member must satisfy constraint: \
                         [Member must have length less than or equal to {MAX_TAG_KEY_LEN}, \
                         Member must have length greater than or equal to {MIN_TAG_KEY_LEN}]"
                    )),
                    "case {case:?}: expected the member-and-bound sentence, got {reply:?}"
                );
            }
        }

        // Accepted or refused, none of these calls names a key the pipe carries — except
        // the row that pairs one with an illegal key, which must remove nothing because
        // the whole call is refused.
        assert_tags_agree(&router, case, name, &tag_map(&stored)).await;
    }
}
