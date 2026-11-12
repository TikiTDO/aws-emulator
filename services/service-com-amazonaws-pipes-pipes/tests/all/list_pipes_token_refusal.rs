// SPDX-License-Identifier: CC0-1.0

//! What `ListPipes` answers for a continuation token it cannot interpret, end to end over
//! the wire.
//!
//! The one rule this suite pins: a request PARAMETER this service cannot interpret at all
//! is refused with `ValidationException` (400) whose `message` is ONE sentence naming the
//! parameter and the reason class — `Parameter <Member> is not valid. Reason: Malformed
//! request.` — with the per-field detail in the modelled `fieldList` and nothing else
//! inside `message`.
//!
//! `tests/all/pipe_refusal_bodies.rs` pins that rule over the FORM of the unreadable
//! token; the tables here span the dimensions it leaves open:
//!
//! * the LENGTH of the token, at both published bounds of `NextToken` and one character
//!   past the maximum, where the front door's own constraint refusal takes over;
//! * what ELSE the listing asks for — every other query member, in either order, and a
//!   store with pipes and one without — so the sentence names the token and not whichever
//!   member happened to travel beside it;
//! * that the refusal is a pure read: it neither moves paging on nor changes what a later
//!   listing (resumed with a token this service DID issue) sees.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, the one path a
//! socket-served request takes, reusing the harness `pipe_lifecycle` owns.

use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_lifecycle::{call, create, listed_names, minimal_body, router, Reply, REGION};
use crate::pipe_refusal_bodies::field_list;

const SOURCE_SQS: &str = "arn:aws:sqs:us-west-2:000000000000:orders";
const TARGET_LAMBDA: &str = "arn:aws:lambda:us-west-2:000000000000:function:handler";

/// The published bounds of the continuation token. Source:
/// `smithy_query(query="shape",shape="com.amazonaws.pipes#NextToken")` over
/// `services/service-com-amazonaws-pipes-pipes/smithy-model/com-amazonaws-pipes-pipes.json`
/// — `@length(min: 1, max: 2048)`, and `@sensitive`, which is why no refusal below may
/// echo the value back.
const MIN_TOKEN: usize = 1;
const MAX_TOKEN: usize = 2048;

/// The sentence an uninterpretable parameter is refused with, spelled out here rather than
/// read from the service, so what the table pins is the wording and not the code that
/// built it: fixed text around the name of the rejected parameter.
fn invalid_parameter_sentence(member: &str) -> String {
    format!("Parameter {member} is not valid. Reason: Malformed request.")
}

/// Assert that `refused` is the one invalid-parameter refusal for `NextToken`: the status,
/// the error code, the sentence, the modelled per-field detail, and that neither the
/// sentence nor that detail carries the service's own field-list document or echoes the
/// token a caller sent.
fn assert_token_refused(case: &str, refused: &Reply, token: &str) {
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {refused:?}"
    );
    assert_eq!(
        refused.message(),
        invalid_parameter_sentence("NextToken"),
        "case {case:?}: expected the one invalid-parameter sentence, got {:?}",
        refused.message()
    );
    assert!(
        !refused.message().contains("ValidationErrorFields"),
        "case {case:?}: the per-field detail belongs in fieldList, not in message, got {:?}",
        refused.message()
    );
    let fields = field_list(refused);
    assert_eq!(
        fields.len(),
        1,
        "case {case:?}: expected one fieldList entry for the refused parameter, got {fields:?}"
    );
    assert_eq!(
        fields[0].0, "NextToken",
        "case {case:?}: expected the fieldList entry to name NextToken, got {fields:?}"
    );
    assert!(
        !fields[0].1.is_empty(),
        "case {case:?}: expected the fieldList entry to say why, got {fields:?}"
    );
    // Only a token long enough to be more than a letter of ordinary English says anything
    // about echoing: a one-digit token is a substring of any sentence that happens to
    // contain that letter.
    if token.len() > "Reason".len() {
        for text in [refused.message(), fields[0].1.clone()] {
            assert!(
                !text.contains(token),
                "case {case:?}: a refusal must not echo the sensitive token, got {text:?}"
            );
        }
    }
}

/// Create `names` as pipes on `router`, so a listing has something to page over.
async fn fixture_pipes(router: &MultiServiceRouter, names: &[&str]) {
    for name in names {
        let created = create(router, name, minimal_body(SOURCE_SQS, TARGET_LAMBDA)).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the fixture pipe must be created, got {created:?}"
        );
    }
}

/// RULE: a continuation token this service cannot read is refused with the one
/// invalid-parameter sentence naming `NextToken` at EVERY length the model lets such a
/// token arrive at; one character past the published maximum the value never reaches the
/// operation at all, so the front door's own constraint refusal — the member alone, which
/// `front_door_refusals` owns — is what answers instead.
///
/// The rows differ along the token's LENGTH (both published bounds, just inside the
/// maximum, and one past it) and along why it cannot be read at that length (not the
/// encoding at all, an odd number of encoding digits, an encoding of bytes that are not
/// text).
#[tokio::test]
async fn an_unreadable_token_is_refused_the_same_way_at_every_published_length() {
    let router = router();
    fixture_pipes(&router, &["alpha", "beta", "gamma"]).await;

    // (case, the `NextToken` value the row sends).
    let rows: Vec<(String, String)> = vec![
        (
            format!("the published minimum of {MIN_TOKEN} character"),
            "z".repeat(MIN_TOKEN),
        ),
        (
            "one digit of the encoding, so an odd number of them".to_string(),
            "a".to_string(),
        ),
        (
            "an odd number of encoding digits".to_string(),
            "a1b".to_string(),
        ),
        (
            "an encoding of bytes that are not text".to_string(),
            "fffe".to_string(),
        ),
        (
            "just inside the published maximum".to_string(),
            "z".repeat(MAX_TOKEN - 1),
        ),
        (
            format!("the published maximum of {MAX_TOKEN} characters"),
            "z".repeat(MAX_TOKEN),
        ),
        (
            "the published maximum of encoding digits, all non-text bytes".to_string(),
            "ff".repeat(MAX_TOKEN / 2),
        ),
    ];

    for (case, token) in rows {
        let refused = call(
            &router,
            "GET",
            &format!("/v1/pipes?NextToken={token}"),
            None,
            REGION,
        )
        .await;
        assert_token_refused(&case, &refused, &token);
    }

    // One character PAST the published maximum: a different class of refusal, because the
    // value is outside a modelled constraint rather than merely unreadable.
    let past_bound = "z".repeat(MAX_TOKEN + 1);
    let refused = call(
        &router,
        "GET",
        &format!("/v1/pipes?NextToken={past_bound}"),
        None,
        REGION,
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "case {:?}: expected 400 ValidationException, got {refused:?}",
        "one past the published maximum"
    );
    assert_ne!(
        refused.message(),
        invalid_parameter_sentence("NextToken"),
        "case {:?}: a value outside the modelled bound is the front door's refusal, \
         not the operation's unreadable-parameter sentence, got {:?}",
        "one past the published maximum",
        refused.message()
    );
}

/// RULE: the same refusal, whatever ELSE the listing asks for — the sentence names the
/// parameter the service could not interpret, so no other query member, no ordering of
/// them and no contents of the store changes it.
///
/// The rows differ along which other query members travel beside the token (the page
/// limit, each of the three prefix filters, each of the two state filters, several at
/// once), along where the token sits among them, and along whether the account holds any
/// pipes at all.
#[tokio::test]
async fn the_token_refusal_does_not_depend_on_the_rest_of_the_listing() {
    let token = "not-a-token";

    // (case, the query string after `/v1/pipes`).
    let rows: Vec<(&str, String)> = vec![
        ("the token alone", format!("?NextToken={token}")),
        ("a limit before it", format!("?Limit=2&NextToken={token}")),
        ("a limit after it", format!("?NextToken={token}&Limit=2")),
        (
            "a name prefix that matches a stored pipe",
            format!("?NamePrefix=alpha&NextToken={token}"),
        ),
        (
            "a name prefix that matches nothing",
            format!("?NamePrefix=nosuch&NextToken={token}"),
        ),
        (
            "a source prefix",
            format!("?SourcePrefix=arn:aws:sqs&NextToken={token}"),
        ),
        (
            "a target prefix",
            format!("?TargetPrefix=arn:aws:lambda&NextToken={token}"),
        ),
        (
            "a current-state filter",
            format!("?CurrentState=RUNNING&NextToken={token}"),
        ),
        (
            "a desired-state filter",
            format!("?DesiredState=STOPPED&NextToken={token}"),
        ),
        (
            "every other member at once, the token last",
            format!(
                "?Limit=100&NamePrefix=a&SourcePrefix=arn&TargetPrefix=arn\
                 &CurrentState=RUNNING&DesiredState=RUNNING&NextToken={token}"
            ),
        ),
    ];

    // The same rows over a store that holds pipes and over one that holds none, so what is
    // refused is the token and not the paging it would have resumed.
    for (store, names) in [
        ("a store holding pipes", &["alpha", "beta", "gamma"][..]),
        ("an empty store", &[][..]),
    ] {
        let router = router();
        fixture_pipes(&router, names).await;
        for (case, query) in &rows {
            let refused = call(&router, "GET", &format!("/v1/pipes{query}"), None, REGION).await;
            assert_token_refused(&format!("{store}, {case}"), &refused, token);
        }
    }
}

/// RULE: refusing a listing is a pure read — the refused call reports nothing of the store
/// and changes nothing in it, so a later listing, including one resumed with a token this
/// service DID issue, sees exactly what it would have seen without the refusals.
///
/// The rows differ along the form of the tampering applied to a token this service issued
/// (a digit dropped, a digit appended, the whole token replaced by text), each interleaved
/// with a page the issued token resumes.
#[tokio::test]
async fn a_refused_token_leaves_later_listings_untouched() {
    let router = router();
    let names = ["alpha", "beta", "gamma", "delta"];
    fixture_pipes(&router, &names).await;

    let all = call(&router, "GET", "/v1/pipes", None, REGION).await;
    assert_eq!(all.status, 200, "the full listing must succeed, got {all:?}");
    let before = listed_names(&all);
    assert_eq!(
        before.len(),
        names.len(),
        "the full listing must report every fixture pipe, got {before:?}"
    );

    let first = call(&router, "GET", "/v1/pipes?Limit=2", None, REGION).await;
    assert_eq!(
        first.status, 200,
        "the first page must succeed, got {first:?}"
    );
    let issued = first.string("NextToken");
    assert!(
        !issued.is_empty(),
        "a listing with a later page must issue a NextToken, got {first:?}"
    );
    let first_page = listed_names(&first);

    // (case, the tampered token the row sends).
    let rows: Vec<(&str, String)> = vec![
        (
            "the issued token with its last digit dropped",
            issued[..issued.len() - 1].to_string(),
        ),
        (
            "the issued token with a digit appended",
            format!("{issued}z"),
        ),
        ("plain text in place of the token", "page-two".to_string()),
    ];

    for (case, tampered) in rows {
        let refused = call(
            &router,
            "GET",
            &format!("/v1/pipes?Limit=2&NextToken={tampered}"),
            None,
            REGION,
        )
        .await;
        assert_token_refused(case, &refused, &tampered);
        assert!(
            refused.member("Pipes").is_null(),
            "case {case:?}: a refusal must report no pipes, got {refused:?}"
        );

        // The token the service issued still resumes the SAME page after the refusal, and
        // the full listing still reports the same names in the same order.
        let resumed = call(
            &router,
            "GET",
            &format!("/v1/pipes?Limit=2&NextToken={issued}"),
            None,
            REGION,
        )
        .await;
        assert_eq!(
            resumed.status, 200,
            "case {case:?}: a token this service issued must still be accepted, got {resumed:?}"
        );
        let resumed_names = listed_names(&resumed);
        assert!(
            resumed_names
                .iter()
                .all(|name| !first_page.contains(name)),
            "case {case:?}: the resumed page must continue past {first_page:?}, \
             got {resumed_names:?}"
        );
        let again = call(&router, "GET", "/v1/pipes", None, REGION).await;
        assert_eq!(
            listed_names(&again),
            before,
            "case {case:?}: a refused listing must change nothing a later listing sees, \
             got {again:?}"
        );
    }
}
