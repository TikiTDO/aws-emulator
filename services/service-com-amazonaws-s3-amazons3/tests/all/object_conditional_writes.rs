// SPDX-License-Identifier: CC0-1.0

//! A write made CONDITIONAL on what the key holds: served when the condition holds, refused
//! when it does not, and storing nothing either way when it does not.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, error code, message) and what the key holds afterwards.
//!
//! The rule under test, from the members that carry the condition:
//!
//! * "Uploads the object only if the ETag (entity tag) value provided during the WRITE
//!   operation matches the ETag of the object in S3. If the ETag values do not match, the
//!   operation returns a 412 Precondition Failed error" (`PutObjectRequest$IfMatch`) — a tag
//!   is the tag OF an object, so a key holding no live object has none to compare and the
//!   write is refused `NoSuchKey` (404): what the condition named is not there at all.
//! * "Uploads the object only if the object key name does not already exist in the bucket
//!   specified. Otherwise, Amazon S3 returns a 412 Precondition Failed error"
//!   (`PutObjectRequest$IfNoneMatch`).
//!
//! A precondition that does not hold carries the code and the sentence Amazon S3's list of
//! error codes publishes for it, and a refused write leaves the object the key holds exactly
//! as it was — which is what makes the pair usable as a compare-and-swap.
//!
//! The rows differ along what the rule generalises over: WHICH member carries the condition,
//! whether the key holds an object at all, whether the tag names the object the key holds,
//! how the tag is QUOTED (a client may send it with or without the double quotes HTTP's
//! quoted-string syntax wraps it in), and which write the condition is on — the one-shot
//! write and the completion of a multipart upload are judged by the same rule.

use service_com_amazonaws_s3_amazons3::state::{
    NO_SUCH_KEY_CODE, NO_SUCH_KEY_MESSAGE, PRECONDITION_FAILED_CODE, PRECONDITION_FAILED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_partial_reads::{multipart_object, seeded_payload};

/// How a row names the entity tag it makes its write conditional on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    /// The tag the key's object really carries, as the read reported it (with its quotes).
    Held,
    /// That same tag with the quotes stripped — the same tag by value.
    HeldUnquoted,
    /// A well-formed tag that is not the held object's.
    Other,
    /// The `*` `If-None-Match` expects.
    Asterisk,
}

impl Tag {
    /// The header value this names, given the tag the key's object carries.
    fn value(self, held: &str) -> String {
        match self {
            Self::Held => held.to_string(),
            Self::HeldUnquoted => held.trim_matches('"').to_string(),
            Self::Other => String::from("\"00000000000000000000000000000000\""),
            Self::Asterisk => String::from("*"),
        }
    }
}

/// The ETag the object at `key` carries, or `None` when the key holds nothing.
async fn held_etag(router: &MultiServiceRouter, bucket_name: &str, key: &str) -> Option<String> {
    let read = get_object(router, bucket_name, key, "", &[]).await;
    (read.status == 200).then(|| read.header("etag")).flatten()
}

/// `length` bytes from `seed` — see [`seeded_payload`].
fn sized_payload(seed: u32, length: usize) -> Vec<u8> {
    seeded_payload(seed, length)
}

/// Complete a multipart upload of `parts` at `key`, with `headers` on the COMPLETION (which
/// is where its conditional members travel) — through the one multipart harness this test
/// crate owns ([`multipart_object`]). Answers what the completion answered.
async fn complete_upload(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    parts: &[Vec<u8>],
    headers: &[(&str, &str)],
) -> Answer {
    multipart_object(router, name, key, parts, &[], headers).await
}

#[tokio::test]
async fn a_conditional_write_is_served_when_it_holds_and_refused_when_it_does_not() {
    let router = router();
    bucket(&router, "conditional-bucket").await;

    // The rows: the member carrying the condition, the tag it names, whether the key holds
    // an object when the write is made, and whether the condition holds.
    let rows: [(&str, &str, Tag, bool, bool); 8] = [
        ("If-Match naming the tag the key holds", "if-match", Tag::Held, true, true),
        (
            "If-Match naming that tag unquoted",
            "if-match",
            Tag::HeldUnquoted,
            true,
            true,
        ),
        (
            "If-Match naming another object's tag",
            "if-match",
            Tag::Other,
            true,
            false,
        ),
        (
            "If-Match on a key that holds nothing",
            "if-match",
            Tag::Other,
            false,
            false,
        ),
        (
            "If-Match naming the tag a key that holds nothing would have",
            "if-match",
            Tag::Held,
            false,
            false,
        ),
        (
            "If-None-Match on a key that holds nothing",
            "if-none-match",
            Tag::Asterisk,
            false,
            true,
        ),
        (
            "If-None-Match on a key that holds an object",
            "if-none-match",
            Tag::Asterisk,
            true,
            false,
        ),
        (
            "If-None-Match naming a tag on a key that holds an object",
            "if-none-match",
            Tag::Other,
            true,
            false,
        ),
    ];

    for (index, (case, member, tag, key_holds_an_object, holds)) in rows.iter().enumerate() {
        // One key per row, so the state each row needs is its own: the rows that need the
        // key to hold an object stage one, and the rest leave the key empty.
        let key = format!("row-{index}.bin");
        let staged = sized_payload(index as u32 + 1, 64);
        if *key_holds_an_object {
            let written = put_object(&router, "conditional-bucket", &key, &staged, &[]).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: the staged object must be written first, got {} with body {:?}",
                written.status,
                text(&written)
            );
        }
        // The tag the condition names: the one the key really holds for the rows that staged
        // an object, and the tag that object WOULD have had for the rows that did not — so
        // `Tag::Held` on an empty key names a well-formed tag that still matches nothing.
        let held = held_etag(&router, "conditional-bucket", &key)
            .await
            .unwrap_or_else(|| String::from("\"d41d8cd98f00b204e9800998ecf8427e\""));
        let value = tag.value(&held);

        let replacement = sized_payload(1_000 + index as u32, 128);
        let answer = put_object(
            &router,
            "conditional-bucket",
            &key,
            &replacement,
            &[(member, value.as_str())],
        )
        .await;

        let read = get_object(&router, "conditional-bucket", &key, "", &[]).await;
        if *holds {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a condition that holds must be served, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                read.body.as_ref(),
                replacement.as_slice(),
                "case {case:?}: the write must have landed, got {} bytes",
                read.body.len()
            );
        } else {
            // WHICH refusal depends on why the condition does not hold: an `If-Match` at a
            // key holding no live object names the entity tag of nothing at all, so what the
            // caller is told is that the key is not there (404 `NoSuchKey`); every other
            // condition that does not hold is the 412 of a failed precondition.
            let (status, code, message) = match (*member, *key_holds_an_object) {
                ("if-match", false) => (404, NO_SUCH_KEY_CODE, NO_SUCH_KEY_MESSAGE),
                _ => (412, PRECONDITION_FAILED_CODE, PRECONDITION_FAILED_MESSAGE),
            };
            assert_eq!(
                answer.status, status,
                "case {case:?}: a condition that does not hold must be refused with HTTP \
                 {status}, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case:?}: expected the {code} code, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                message,
                "case {case:?}: expected the published sentence, got {:?}",
                answer.error_message()
            );
            if *key_holds_an_object {
                assert_eq!(
                    read.body.as_ref(),
                    staged.as_slice(),
                    "case {case:?}: a refused write must leave the object as it was, got {} \
                     bytes",
                    read.body.len()
                );
            } else {
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: a refused write must store nothing, got {}",
                    read.status
                );
            }
        }
    }
}

#[tokio::test]
async fn a_compare_and_swap_loop_sees_its_own_write_and_loses_a_stale_one() {
    let router = router();
    bucket(&router, "swap-bucket").await;
    let key = "counter.bin";

    // The primitive a client uses this pair for: create the key only if it does not exist,
    // then replace it only if it still holds what the client last read. A second write
    // against the tag the first one replaced is the lost race, and it is refused.
    let created = put_object(
        &router,
        "swap-bucket",
        key,
        b"first",
        &[("if-none-match", "*")],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the create must be served while the key holds nothing, got {} with body {:?}",
        created.status,
        text(&created)
    );
    let first_tag = held_etag(&router, "swap-bucket", key)
        .await
        .expect("the created object must report an ETag");

    let swapped = put_object(
        &router,
        "swap-bucket",
        key,
        b"second",
        &[("if-match", first_tag.as_str())],
    )
    .await;
    assert_eq!(
        swapped.status, 200,
        "the swap against the tag the key holds must be served, got {} with body {:?}",
        swapped.status,
        text(&swapped)
    );

    let stale = put_object(
        &router,
        "swap-bucket",
        key,
        b"third",
        &[("if-match", first_tag.as_str())],
    )
    .await;
    assert_eq!(
        stale.status, 412,
        "a swap against the tag that was replaced must be refused, got {} with body {:?}",
        stale.status,
        text(&stale)
    );
    assert_eq!(
        stale.error_code(),
        PRECONDITION_FAILED_CODE,
        "expected the PreconditionFailed code, got {:?}",
        stale.error_code()
    );
    let read = get_object(&router, "swap-bucket", key, "", &[]).await;
    assert_eq!(
        read.body.as_ref(),
        b"second",
        "the lost write must have changed nothing, got {:?}",
        text(&read)
    );
}

#[tokio::test]
async fn the_completion_of_a_multipart_upload_is_judged_by_the_same_rule() {
    let router = router();
    bucket(&router, "conditional-mpu").await;
    // ONE part, which "there is no minimum size limit on" because it is the last part of the
    // upload — the condition is about what the key holds, not about how many parts the
    // upload assembled.
    let parts = vec![sized_payload(7, 1_024)];
    let assembled: Vec<u8> = parts.concat();

    // A key holding nothing has no entity tag to match, so the completion's `If-Match` is
    // refused `NoSuchKey` — the same answer the one-shot write gives, which is what makes
    // this one rule rather than two.
    let absent = complete_upload(
        &router,
        "conditional-mpu",
        "assembled.bin",
        &parts,
        &[("if-match", "\"00000000000000000000000000000000\"")],
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case \"If-Match onto a key holding nothing\": expected HTTP 404, got {} with body \
         {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        NO_SUCH_KEY_CODE,
        "case \"If-Match onto a key holding nothing\": expected the {NO_SUCH_KEY_CODE} code, \
         got {:?}",
        absent.error_code()
    );
    assert_eq!(
        absent.error_message(),
        NO_SUCH_KEY_MESSAGE,
        "case \"If-Match onto a key holding nothing\": expected the published sentence, got \
         {:?}",
        absent.error_message()
    );

    // A key holding nothing: `If-None-Match: *` holds, so the completion lands.
    let created = complete_upload(
        &router,
        "conditional-mpu",
        "assembled.bin",
        &parts,
        &[("if-none-match", "*")],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the completion must be served while the key holds nothing, got {} with body {:?}",
        created.status,
        text(&created)
    );
    let held = held_etag(&router, "conditional-mpu", "assembled.bin")
        .await
        .expect("the completed object must report an ETag");

    // The same two refusals the one-shot write answers, on the same key: the condition is
    // judged against what the key holds at the moment of the write.
    for (case, member, value) in [
        ("If-None-Match on a key that holds an object", "if-none-match", String::from("*")),
        (
            "If-Match naming another object's tag",
            "if-match",
            String::from("\"00000000000000000000000000000000\""),
        ),
    ] {
        let refused = complete_upload(
            &router,
            "conditional-mpu",
            "assembled.bin",
            &parts,
            &[(member, value.as_str())],
        )
        .await;
        assert_eq!(
            refused.status, 412,
            "case {case:?}: expected HTTP 412, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_message(),
            PRECONDITION_FAILED_MESSAGE,
            "case {case:?}: expected the published sentence, got {:?}",
            refused.error_message()
        );
    }

    // And the condition that HOLDS is served: `If-Match` naming the tag the key holds, sent
    // unquoted, which is the same tag by value.
    let swapped = complete_upload(
        &router,
        "conditional-mpu",
        "assembled.bin",
        &parts,
        &[("if-match", held.trim_matches('"'))],
    )
    .await;
    assert_eq!(
        swapped.status, 200,
        "the completion against the tag the key holds must be served, got {} with body {:?}",
        swapped.status,
        text(&swapped)
    );
    let read = get_object(&router, "conditional-mpu", "assembled.bin", "", &[]).await;
    assert_eq!(
        read.body.as_ref(),
        assembled.as_slice(),
        "the completed object must hold the assembled bytes, got {} of them",
        read.body.len()
    );
}
