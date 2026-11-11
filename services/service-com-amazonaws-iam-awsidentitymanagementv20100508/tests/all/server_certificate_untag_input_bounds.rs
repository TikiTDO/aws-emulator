// SPDX-License-Identifier: CC0-1.0

//! What `UntagServerCertificate` does with the published bounds of its OWN input members,
//! end to end over the awsQuery wire, read back through `GetServerCertificate` and
//! `ListServerCertificateTags`.
//!
//! RULE under test: the request is taken against the constraints its input members publish
//! BEFORE any tag is removed — a key list whose size is inside `tagKeyListType`'s published
//! length, each key inside `tagKeyType`'s published length and alphabet, and a
//! `ServerCertificateName` inside `serverCertificateNameType`'s published length and
//! alphabet are served and remove exactly the held keys the call names, while a value
//! outside one of those is refused with HTTP 400 and the error code this service answers a
//! value it cannot accept with, leaving the persisted tag set exactly as it was. The bounds
//! come from the model: `TagKeys` targets `tagKeyListType` (length 0..50) whose member
//! targets `tagKeyType` (length 1..128, pattern `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`) and
//! `ServerCertificateName` targets `serverCertificateNameType` (length 1..128, pattern
//! `^[\w+=,.@-]+$`) (`smithy_query(query="operation",
//! operation="UntagServerCertificate")` and `smithy_query(query="shape",
//! shape="tagKeyListType")`).
//!
//! `tests/all/server_certificate_tag_removal.rs` holds the MEMBERSHIP axis of the same
//! operation — which keys survive a removal, how keys compare, the certificate's name
//! casing and the per-resource quota — and owns the untag call this suite drives;
//! `tests/all/server_certificate_untag_listing.rs` holds the reported-list axis. The axis
//! here is the INPUT itself: which requests reach the removal at all, and that a request
//! that does not reach it changes nothing.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, `Message`) of a certificate that was
//! PERSISTED rather than a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns, the quota fixture is `tests/all/role_tag_removal.rs`'s,
//! the tag-list builders are `tests/all/role_tags.rs`'s and the certificate upload and both
//! tag readers are `tests/all/server_certificate_tag_updates.rs`'s, so a request is built
//! and read exactly one way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over: which
//! member carries the constraint, whether it is a length on the list, a length on one of
//! its keys or the form of the addressed name, which end of the bound is taken, by how much
//! it is broken, which position in the list carries the offending key, and the published
//! bound itself.
//!
//! The list's published MINIMUM is zero, and a list of zero keys carries no
//! `TagKeys.member.N` parameter at all on this wire, so it cannot be told from the required
//! member being absent: the last suite reads back the refusal such a request gets.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;

use crate::role_tag_removal::{fixture, keys};
use crate::role_tags::expected_map;
use crate::server_certificate_tag_removal::untag_certificate;
use crate::server_certificate_tag_updates::{assert_reads, upload_certificate};
use crate::user_lifecycle::{element, error_code, router};

/// The error code this service answers a request value it cannot accept with, on the query
/// protocol — the one the boundary walk names and the one a client branches on
/// (`tests/all/range_refusal_wording.rs` owns the rule for the whole service).
const VALUE_REFUSAL_CODE: &str = "ValidationError";

/// The longest key a stored tag can be named by, from `tagKeyType`'s published length.
const MAX_TAG_KEY_CHARACTERS: usize = 128;

/// The longest name a server certificate can be addressed by, from
/// `serverCertificateNameType`'s published length.
const MAX_CERTIFICATE_NAME_CHARACTERS: usize = 128;

/// The envelope the boundary walk states for ONE broken requirement whose value this
/// service does not echo — a value outside a bound its member declares.
fn one_unechoed_violation(member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// The envelope the boundary walk states for ONE broken requirement whose value it KEEPS:
/// a value reached INSIDE one of the input's collections, which the restating leaves the
/// framework's own clause for — the value it carried and the model's expression as written
/// (`src/service/refusal_clauses.rs`'s `pattern_clause`, which restates a member reached at
/// the top of an operation's input and returns the framework's clause for a nested one).
fn one_echoed_violation(member: &str, value: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// The requirement clause a value outside its member's `@pattern` states, quoting
/// `expression`: the expression the model declares, as written for a value reached inside a
/// collection, and without the pair of anchors the model writes around a whole-value one for
/// a member of the input itself
/// (`src/service/refusal_clauses.rs`, whose rule `tests/all/range_refusal_wording.rs` owns).
fn outside_the_pattern(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The requirement clause a value over a declared `@length` maximum states.
fn above_length(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// `count` tag keys that no fixture certificate holds, so a row can name more keys than any
/// certificate carries without the size under test depending on what is held.
fn unheld_keys(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("unheld-{index:02}")).collect()
}

/// Assert `status` and `body` are the refusal this service answers a value it cannot accept
/// with, carrying exactly `expected` as its message.
fn assert_value_refusal(case: &str, status: u16, body: &str, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a value outside a published constraint, got \
         {status}: {body}"
    );
    let code = error_code(body);
    assert_eq!(
        code, VALUE_REFUSAL_CODE,
        "case {case:?}: expected the error code {VALUE_REFUSAL_CODE}, got {code:?}: {body}"
    );
    let message = element(body, "Message");
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
}

/// RULE: the SIZE of the key list is taken against `tagKeyListType`'s published length, so
/// a list of any size inside it is served and removes exactly the held keys it names, while
/// a list past the published maximum is refused and removes nothing.
#[tokio::test]
async fn the_published_key_list_length_is_served_and_a_longer_list_is_refused() {
    // The rows start from a certificate at the per-resource tag quota, which is the largest
    // held set a single call's keys can be drawn from.
    let held = fixture(MAX_TAGS_PER_ENTITY);
    let held_keys: Vec<String> = held.iter().map(|(key, _)| key.clone()).collect();
    let list_maximum = 50;

    // One row per size inside the published length: one key, a few, half the maximum and the
    // maximum itself. The published MINIMUM is zero, and a list of zero keys carries no
    // `TagKeys.member.N` parameter at all on this wire, so it is indistinguishable from the
    // required member being absent — which is the refusal the last suite below reads back.
    let served = [
        ("one key", 1usize),
        ("two keys", 2),
        ("a few keys", 5),
        ("half the published maximum", list_maximum / 2),
        ("one key below the published maximum", list_maximum - 1),
        ("the published maximum", list_maximum),
    ];

    let router = router();
    for (case, size) in served {
        let certificate_name = format!("list-size-{size}");
        upload_certificate(&router, &certificate_name, None, &held).await;

        let named = held_keys[..size].to_vec();
        let (status, body) = untag_certificate(&router, &certificate_name, &named).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a list of {size} key(s) is inside the published length and must \
             be served, got {status}: {body}"
        );

        // The survivors are the held set less exactly the keys the call named.
        let expected: BTreeMap<String, String> = expected_map(&held[size..]);
        assert_reads(&router, case, &certificate_name, &expected).await;
    }

    // One past the published maximum, and far past it: refused by the boundary walk, which
    // states the list's member and its bound and nothing of the list's own size.
    let refused = [
        ("one key past the published maximum", list_maximum + 1),
        ("far past the published maximum", list_maximum * 2),
    ];
    for (case, size) in refused {
        let certificate_name = format!("list-over-{size}");
        upload_certificate(&router, &certificate_name, None, &held).await;

        // The held keys first, then invented ones, so the only thing wrong with the request
        // is how many keys it carries.
        let mut named = held_keys.clone();
        named.extend(unheld_keys(size - held_keys.len()));
        let (status, body) = untag_certificate(&router, &certificate_name, &named).await;
        assert_value_refusal(
            case,
            status,
            &body,
            &one_unechoed_violation("tagKeys", &above_length(list_maximum)),
        );
        assert!(
            !body.contains(&format!("{size} items")),
            "case {case:?}: the clause must not echo the list's size, got {body}"
        );

        // Nothing was removed: the certificate still holds everything its upload stored.
        assert_reads(&router, case, &certificate_name, &expected_map(&held)).await;
    }
}

/// One row of the per-key table: the keys one call names, and the whole message the refusal
/// must carry. Every row's list size is inside the published list length — the KEY is what
/// is under test here.
struct KeyCase {
    case: String,
    named: Vec<String>,
    expected: String,
}

/// The rows: a key over the published key length, at each position a list can carry it in,
/// and a key outside the published key alphabet.
fn key_cases() -> Vec<KeyCase> {
    let one_over = "k".repeat(MAX_TAG_KEY_CHARACTERS + 1);
    let far_over = "k".repeat(MAX_TAG_KEY_CHARACTERS * 4);
    // `tagKeyType`'s pattern admits letters, separators, numbers and `_.:/=+-@`
    // (`smithy_query(query="shape", shape="tagKeyType")`), so each of these carries one
    // character from outside that class — including `,`, which the ENTITY-name alphabet
    // admits and this one does not.
    let outside_the_alphabet = ["no*stars", "no%percent", "no,commas", "no#hashes"];

    let mut rows = vec![
        KeyCase {
            case: "a key one character over the published length".to_string(),
            named: vec![one_over.clone()],
            expected: one_unechoed_violation(
                "tagKeys.1.member",
                &above_length(MAX_TAG_KEY_CHARACTERS),
            ),
        },
        KeyCase {
            case: "a key far over the published length".to_string(),
            named: vec![far_over],
            expected: one_unechoed_violation(
                "tagKeys.1.member",
                &above_length(MAX_TAG_KEY_CHARACTERS),
            ),
        },
        // The clause names the POSITION the offending key arrived at, so the same key is
        // stated against a different path further down the list.
        KeyCase {
            case: "an over-long key second in the list".to_string(),
            named: vec!["key-00".to_string(), one_over.clone()],
            expected: one_unechoed_violation(
                "tagKeys.2.member",
                &above_length(MAX_TAG_KEY_CHARACTERS),
            ),
        },
        KeyCase {
            case: "an over-long key third in the list".to_string(),
            named: vec!["key-00".to_string(), "key-01".to_string(), one_over],
            expected: one_unechoed_violation(
                "tagKeys.3.member",
                &above_length(MAX_TAG_KEY_CHARACTERS),
            ),
        },
    ];
    for key in outside_the_alphabet {
        rows.push(KeyCase {
            case: format!("a key carrying {key:?}, outside the published alphabet"),
            named: vec![key.to_string()],
            // A tag key is a value reached INSIDE the key list, so the restating leaves it
            // the framework's own clause: the value the request carried, and the model's
            // expression as written. A member of the input itself is the one stated with
            // neither (`src/service/refusal_clauses.rs`).
            expected: one_echoed_violation(
                "tagKeys.1.member",
                key,
                &outside_the_pattern(r"^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$"),
            ),
        });
    }
    rows
}

/// RULE: each key in the list is taken against `tagKeyType`'s own published length and
/// alphabet, so a key outside either is refused with the member path naming the position it
/// arrived at, and no key of that request — not even a held one named beside it — is
/// removed.
#[tokio::test]
async fn a_key_outside_its_published_form_is_refused_and_removes_nothing() {
    let held = fixture(4);
    let router = router();

    for (index, KeyCase {
        case,
        named,
        expected,
    }) in key_cases().into_iter().enumerate()
    {
        let certificate_name = format!("key-form-{index}");
        upload_certificate(&router, &certificate_name, None, &held).await;

        let (status, body) = untag_certificate(&router, &certificate_name, &named).await;
        assert_value_refusal(&case, status, &body, &expected);

        // The whole request was refused, so the held keys named beside the offending one
        // are still there.
        assert_reads(&router, &case, &certificate_name, &expected_map(&held)).await;
    }
}

/// RULE: the refusal is about the published form, not a blanket refusal of the member, so a
/// key AT each published bound of `tagKeyType` and one spanning its alphabet is served and
/// removes the tag it names.
#[tokio::test]
async fn each_published_key_bound_itself_is_served() {
    let longest = "k".repeat(MAX_TAG_KEY_CHARACTERS);
    // The published alphabet's own characters, which a served key may be built entirely
    // from: a letter, a separator, a number and `_.:/=+-@`.
    let alphabet = "Cost Center_1.2:3/4=5+6-7@8";
    let served = [
        ("a key at the published minimum length", "k"),
        ("a key at the published maximum length", longest.as_str()),
        ("a key spanning the published alphabet", alphabet),
    ];

    let router = router();
    for (index, (case, key)) in served.iter().enumerate() {
        let certificate_name = format!("key-bound-{index}");
        let held = vec![
            ((*key).to_string(), "removed".to_string()),
            ("kept".to_string(), "value".to_string()),
        ];
        upload_certificate(&router, &certificate_name, None, &held).await;

        let (status, body) = untag_certificate(&router, &certificate_name, &keys(&[key])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a key inside the published form must be served, got {status}: \
             {body}"
        );
        assert_reads(
            &router,
            case,
            &certificate_name,
            &expected_map(&held[1..]),
        )
        .await;
    }
}

/// The whole sentence AWS states for a value outside the character class it publishes for an
/// IAM entity NAME, naming the member it was supplied for and nothing of the value
/// (`UntagServerCertificate$ServerCertificateName`: "a string of characters consisting of
/// upper and lowercase alphanumeric characters with no spaces. You can also include any of
/// the following characters: _+=,.@-").
fn entity_name_refusal(member: &str) -> String {
    format!(
        "The specified value for {member} is invalid. It must contain only alphanumeric \
         characters and/or the following: +=,.@_-"
    )
}

/// RULE: the addressed name is taken against `serverCertificateNameType`'s published length
/// and alphabet too — a name outside the alphabet is refused with AWS's one sentence for
/// that form and a name over the published length with the bound's clause — while a name
/// INSIDE both that the account holds no certificate under is the modelled absence instead,
/// so the two refusals are not confused and neither removes anything.
#[tokio::test]
async fn the_addressed_name_is_taken_against_its_published_form() {
    let held = fixture(3);
    let router = router();
    upload_certificate(&router, "name-form", None, &held).await;

    // Outside the published alphabet: a space, and each of the characters the class leaves
    // out that a path or an ARN would carry.
    let outside_the_alphabet = ["has space", "slash/name", "star*name", "colon:name"];
    for spelling in outside_the_alphabet {
        let (status, body) = untag_certificate(&router, spelling, &keys(&["key-0"])).await;
        assert_value_refusal(
            spelling,
            status,
            &body,
            &entity_name_refusal("serverCertificateName"),
        );
        assert!(
            !body.contains(spelling),
            "case {spelling:?}: the sentence must not echo the refused name, got {body}"
        );
    }

    // Over the published length, with a name the alphabet admits so the length is the only
    // requirement broken.
    for (case, size) in [
        (
            "one character over the published length",
            MAX_CERTIFICATE_NAME_CHARACTERS + 1,
        ),
        (
            "far over the published length",
            MAX_CERTIFICATE_NAME_CHARACTERS * 3,
        ),
    ] {
        let name = "n".repeat(size);
        let (status, body) = untag_certificate(&router, &name, &keys(&["key-0"])).await;
        assert_value_refusal(
            case,
            status,
            &body,
            &one_unechoed_violation(
                "serverCertificateName",
                &above_length(MAX_CERTIFICATE_NAME_CHARACTERS),
            ),
        );
    }

    // A name INSIDE both published constraints that the account holds no certificate under
    // is the modelled absence, not a value refusal — including one at the published
    // maximum length.
    for absent in [
        "n".repeat(MAX_CERTIFICATE_NAME_CHARACTERS),
        "n".to_string(),
        "A+B=C,d@e-f_g".to_string(),
    ] {
        let (status, body) = untag_certificate(&router, &absent, &keys(&["key-0"])).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: a name inside the published form that the account holds no \
             certificate under must be the modelled absence, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "NoSuchEntity",
            "case {absent:?}: expected the modelled awsQuery code, got {code:?}: {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("The Server Certificate with name {absent} cannot be found."),
            "case {absent:?}: expected the message to name the certificate, got {message:?}"
        );
    }

    // Not one of those refusals removed anything the held certificate carries.
    assert_reads(&router, "every refusal", "name-form", &expected_map(&held)).await;
}

/// RULE: `TagKeys` is a REQUIRED member, and a list of no keys carries no
/// `TagKeys.member.N` parameter on this wire, so a request that names no key at all is a
/// request whose required member is absent: it is refused before the removal, with the
/// clause naming that member, and the held set is left exactly as it was.
#[tokio::test]
async fn a_request_naming_no_key_is_refused_before_anything_is_removed() {
    let held = fixture(2);
    let router = router();
    upload_certificate(&router, "no-keys", None, &held).await;

    let (status, body) = untag_certificate(&router, "no-keys", &[]).await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 for a request naming no tag key at all, got {status}: {body}"
    );
    let message = element(&body, "Message");
    assert_eq!(
        message,
        "1 validation error detected: Value null at 'tagKeys' failed to satisfy constraint: \
         Member must not be null",
        "expected the clause to name the absent required member, got {message:?}"
    );

    // The refusal removed nothing.
    assert_reads(&router, "no key named", "no-keys", &expected_map(&held)).await;
}

