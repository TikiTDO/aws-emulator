// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteKeyPair` and the `DescribeKeyPairs` read it is observed
//! through, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=DeleteKeyPair`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation REMOVES THE PUBLIC KEY THE ACCOUNT HOLDS FOR
//! THE KEY PAIR THE REQUEST NAMES — "deletes the specified key pair, by removing the public
//! key from Amazon EC2" (`DeleteKeyPair`) — and reports "the ID of the key pair"
//! (`DeleteKeyPairResult$KeyPairId`) it removed beside `Return` true; a request naming a key
//! pair the account holds none of removes nothing and still succeeds, because the account is
//! already in the state the call asked for, and then there is no id to report.
//!
//! Every table row is a distinct input along what that generalises over — which member names
//! the key pair (its id, its name, or both), the key's type, whether it carries tags, the
//! name's length and character set, whether the key pair is held at all, and whether the
//! same delete is repeated — so the one case a recording happens to contain is one row among
//! many rather than the specification. The two published bounds on a key pair name (255
//! characters, ASCII) are each driven at the bound and one past it, and a name differing from
//! a held one only in letter case is its own row: key pair names are compared byte for byte,
//! so such a request deletes nothing.
//!
//! `CreateKeyPair`'s own suite is [`super::key_pairs`] and the import's is
//! [`super::import_key_pair`]; this file covers the DELETE, what it refuses, and what a dry
//! run of it leaves behind.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed, member};
use super::wire::{is_long_id, param, Param};

/// The Region every call here is signed for. A key pair carries no Region member, so the
/// Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// EC2's answer to an authorized dry run, and the status it carries it on — the same three
/// values [`super::dry_run_before_resource_lookup`] pins for the operations that answer the
/// flag before they resolve anything.
const DRY_RUN_CODE: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `DeleteKeyPair` call carrying `params`.
async fn delete(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DeleteKeyPair", params).await
}

/// `DeleteKeyPair` requiring success, and the body it answered.
async fn delete_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = delete(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteKeyPair, got {status} with body {body}"
    );
    assert_eq!(
        member(&body, "return").as_deref(),
        Some("true"),
        "case {case:?}: expected <return>true</return> from DeleteKeyPair, got body {body}"
    );
    body
}

/// Create one key pair and answer its id, the type and tags the row asked for.
async fn create(
    case: &str,
    router: &MultiServiceRouter,
    key_name: &str,
    key_type: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut fields = vec![param("KeyName", key_name), param("KeyType", key_type)];
    if !tags.is_empty() {
        fields.push(param("TagSpecification.1.ResourceType", "key-pair"));
        for (index, (key, value)) in tags.iter().enumerate() {
            fields.push(param(
                &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                key,
            ));
            fields.push(param(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                value,
            ));
        }
    }
    let (status, body) = super::ec2_query::call(router, REGION, "CreateKeyPair", &fields).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateKeyPair, got {status} with body {body}"
    );
    let key_pair_id = element(&body, "keyPairId")
        .unwrap_or_else(|| panic!("case {case:?}: no <keyPairId> from CreateKeyPair in {body}"));
    assert!(
        is_long_id("key", &key_pair_id),
        "case {case:?}: expected a key pair id from CreateKeyPair, got {key_pair_id:?}"
    );
    key_pair_id
}

/// The ids of every key pair the account holds, as an unnarrowed `DescribeKeyPairs` reports
/// them.
async fn described_ids(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeKeyPairs", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeKeyPairs, got {status} with body {body}"
    );
    elements(&body, "keyPairId")
}

/// Assert that neither the name nor the id of a deleted key pair resolves any more: a
/// name-scoped and an id-scoped `DescribeKeyPairs` each refuse with `InvalidKeyPair.NotFound`
/// naming what the request gave them.
async fn assert_gone(case: &str, router: &MultiServiceRouter, key_name: &str, key_pair_id: &str) {
    for (selector, params, expected_message) in [
        (
            "KeyName",
            indexed("KeyName", &[key_name]),
            format!("The key pair '{key_name}' does not exist"),
        ),
        (
            "KeyPairId",
            indexed("KeyPairId", &[key_pair_id]),
            format!("The key pair ID '{key_pair_id}' does not exist"),
        ),
    ] {
        let (status, body) =
            super::ec2_query::call(router, REGION, "DescribeKeyPairs", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a {selector}-scoped read of the deleted key pair must be \
             refused, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidKeyPair.NotFound",
            "case {case:?}: expected InvalidKeyPair.NotFound from the {selector}-scoped \
             read, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: unexpected message from the {selector}-scoped read of the \
             deleted key pair, body {body}"
        );
    }
}

/// How a row names the key pair it deletes: the two members the request may carry, alone or
/// together ("the ID of the key pair" / "the name of the key pair").
#[derive(Debug, Clone, Copy)]
enum Selector {
    Id,
    Name,
    Both,
}

impl Selector {
    /// The form fields naming the key pair `key_name`/`key_pair_id` this way.
    fn fields(self, key_name: &str, key_pair_id: &str) -> Vec<Param> {
        match self {
            Selector::Id => vec![param("KeyPairId", key_pair_id)],
            Selector::Name => vec![param("KeyName", key_name)],
            Selector::Both => vec![param("KeyPairId", key_pair_id), param("KeyName", key_name)],
        }
    }
}

/// One row of the delete table: the case, the key pair's name, its type, its tags, and which
/// member the delete names it with.
type DeleteRow = (
    &'static str,
    String,
    &'static str,
    Vec<(&'static str, &'static str)>,
    Selector,
);

/// RULE: a delete removes the key pair the request names, whichever member names it, and
/// answers `Return` true with the id of the key pair it removed — after which neither that
/// id nor that name resolves, and every other key pair the account holds is untouched.
#[tokio::test]
async fn delete_key_pair_removes_the_named_key_pair_and_reports_its_id() {
    let router = emulator();

    // A key pair no row names, so every row can assert that only what it asked for went.
    let witness_id = create("the witness key pair", &router, "witness", "ed25519", &[]).await;

    // (case, key name, key type, tags, how the delete names it)
    let rows: Vec<DeleteRow> = vec![
        (
            "an ed25519 key pair named by its name",
            "deploy-key".to_string(),
            "ed25519",
            Vec::new(),
            Selector::Name,
        ),
        (
            "an rsa key pair named by its id",
            "ops_key.1".to_string(),
            "rsa",
            Vec::new(),
            Selector::Id,
        ),
        (
            "a tagged key pair named by id and name together",
            "tagged-key".to_string(),
            "ed25519",
            vec![("Owner", "TeamA"), ("Env", "staging")],
            Selector::Both,
        ),
        (
            "a name of one character",
            "k".to_string(),
            "ed25519",
            Vec::new(),
            Selector::Name,
        ),
        (
            "a name of punctuation and digits",
            "key-2026.06_v2/west".to_string(),
            "ed25519",
            Vec::new(),
            Selector::Name,
        ),
        (
            // "Up to 255 ASCII characters" (`CreateKeyPairRequest$KeyName`): AT the bound.
            "a name of the longest published length",
            "n".repeat(255),
            "ed25519",
            Vec::new(),
            Selector::Name,
        ),
    ];

    for (case, key_name, key_type, tags, selector) in rows {
        let key_pair_id = create(case, &router, &key_name, key_type, &tags).await;

        let body = delete_ok(case, &router, &selector.fields(&key_name, &key_pair_id)).await;
        assert_eq!(
            member(&body, "keyPairId").as_deref(),
            Some(key_pair_id.as_str()),
            "case {case:?}: expected the deleted key pair's id {key_pair_id:?} in the \
             response, got body {body}"
        );

        assert_gone(case, &router, &key_name, &key_pair_id).await;

        let held = described_ids(case, &router).await;
        assert!(
            !held.contains(&key_pair_id),
            "case {case:?}: the deleted key pair {key_pair_id:?} must be gone from the \
             store, got {held:?}"
        );
        assert!(
            held.contains(&witness_id),
            "case {case:?}: the key pair {witness_id:?} the row did not name must still be \
             held, got {held:?}"
        );
    }
}

/// RULE: a delete naming a key pair the account holds none of removes nothing and still
/// succeeds — the public key the call asks EC2 to stop holding is already not held — and
/// reports no `KeyPairId`, there being no key pair it removed.
#[tokio::test]
async fn deleting_a_key_pair_the_account_does_not_hold_succeeds_and_reports_no_id() {
    let router = emulator();

    // One key pair is held throughout, so a row naming something else can assert that
    // nothing of the account's went with it.
    let held_name = "held-key";
    let held_id = create("the held key pair", &router, held_name, "ed25519", &[]).await;

    // A key pair created only to be deleted twice: the second delete is the repeat.
    let gone_name = "gone-key";
    let gone_id = create("the repeated delete", &router, gone_name, "ed25519", &[]).await;
    delete_ok(
        "the first delete of the repeated row",
        &router,
        &[param("KeyName", gone_name)],
    )
    .await;

    // (case, the members the delete carries)
    let rows: Vec<(&str, Vec<Param>)> = vec![
        (
            "the same name again, after it was deleted",
            vec![param("KeyName", gone_name)],
        ),
        (
            "the id of the key pair that was deleted",
            vec![param("KeyPairId", &gone_id)],
        ),
        (
            "a name the account never held",
            vec![param("KeyName", "never-created")],
        ),
        (
            // A well-formed long id of this kind that no key pair was ever issued.
            "a well-formed id no key pair was issued",
            vec![param("KeyPairId", "key-0fedcba987654321f")],
        ),
        (
            // The short id form EC2 also issued ids in.
            "a well-formed short id no key pair was issued",
            vec![param("KeyPairId", "key-0fedcba9")],
        ),
        (
            // Key pair names are compared byte for byte, so this names no key pair the
            // account holds even though one differs from it only in letter case.
            "a name differing from a held one only in letter case",
            vec![param("KeyName", "HELD-KEY")],
        ),
        (
            // "Up to 255 ASCII characters" (`CreateKeyPairRequest$KeyName`): AT the bound,
            // and nothing is held under it.
            "a name of the longest published length that is not held",
            vec![param("KeyName", &"z".repeat(255))],
        ),
        (
            // Both members, naming nothing: there is no key pair for them to disagree about.
            "an id and a name that are both unheld",
            vec![
                param("KeyPairId", "key-0fedcba987654321e"),
                param("KeyName", "never-created-either"),
            ],
        ),
        (
            // The ID is what the pair is resolved by, so an unheld id names no key pair
            // whatever name stands beside it — including the name of one the account does
            // hold, which this row pins rather than leaving incidental.
            "an unheld id beside the name of a held key pair",
            vec![
                param("KeyPairId", "key-0fedcba987654321d"),
                param("KeyName", held_name),
            ],
        ),
    ];

    for (case, params) in rows {
        let body = delete_ok(case, &router, &params).await;
        assert_eq!(
            member(&body, "keyPairId"),
            None,
            "case {case:?}: a delete that removed no key pair must report no keyPairId, \
             got body {body}"
        );

        let held = described_ids(case, &router).await;
        assert_eq!(
            held,
            vec![held_id.clone()],
            "case {case:?}: only the key pair no row named must still be held, got {held:?}"
        );
    }

    // The repeated delete's key pair stayed gone, and the held one is still readable under
    // its own name and id.
    assert_gone("the repeated delete", &router, gone_name, &gone_id).await;
    let held = described_ids("the held key pair", &router).await;
    assert_eq!(
        held,
        vec![held_id.clone()],
        "the held key pair {held_id:?} must survive every delete of something else, got \
         {held:?}"
    );
}

/// RULE: a delete whose own members cannot be read is refused on the REQUEST's terms — a
/// request naming neither member, a value that is not a key pair id, a name outside the
/// published bounds, and an id and name that name different key pairs — and removes nothing.
#[tokio::test]
async fn delete_key_pair_refuses_a_request_it_cannot_read() {
    let router = emulator();

    let held_name = "held-key";
    let held_id = create("the held key pair", &router, held_name, "ed25519", &[]).await;
    let other_name = "other-key";
    let other_id = create("the second held key pair", &router, other_name, "ed25519", &[]).await;

    // (case, the members the delete carries, the code and the message it is refused with)
    let rows: Vec<(&str, Vec<Param>, &str, String)> = vec![
        (
            "neither member",
            Vec::new(),
            "MissingParameter",
            "The request must contain either the parameter KeyName or KeyPairId".to_string(),
        ),
        (
            "an id whose digits are too few",
            vec![param("KeyPairId", "key-123")],
            "InvalidParameterValue",
            "Invalid value 'key-123' for keyPairId.".to_string(),
        ),
        (
            "an id of another resource kind",
            vec![param("KeyPairId", "vpc-0fedcba987654321f")],
            "InvalidParameterValue",
            "Invalid value 'vpc-0fedcba987654321f' for keyPairId.".to_string(),
        ),
        (
            "an id in upper case",
            vec![param("KeyPairId", "key-0FEDCBA987654321F")],
            "InvalidParameterValue",
            "Invalid value 'key-0FEDCBA987654321F' for keyPairId.".to_string(),
        ),
        (
            "an id with no digits at all",
            vec![param("KeyPairId", "key-")],
            "InvalidParameterValue",
            "Invalid value 'key-' for keyPairId.".to_string(),
        ),
        (
            "a key pair name given as the id",
            vec![param("KeyPairId", held_name)],
            "InvalidParameterValue",
            format!("Invalid value '{held_name}' for keyPairId."),
        ),
        (
            // One character past "up to 255 ASCII characters"
            // (`CreateKeyPairRequest$KeyName`).
            "a name one character past the published maximum",
            vec![param("KeyName", &"n".repeat(256))],
            "InvalidParameterValue",
            format!(
                "Value ({}) for parameter keyName is invalid. \
                 A key pair name accepts at most 255 characters",
                "n".repeat(256)
            ),
        ),
        (
            "a name outside ASCII",
            vec![param("KeyName", "clé-de-déploiement")],
            "InvalidParameterValue",
            "Value (clé-de-déploiement) for parameter keyName is invalid. \
             A key pair name accepts ASCII characters only"
                .to_string(),
        ),
        (
            "an id and a name naming two different key pairs",
            vec![param("KeyPairId", &held_id), param("KeyName", other_name)],
            "InvalidParameterCombination",
            format!("The key pair '{held_id}' is not named '{other_name}'"),
        ),
        (
            "an id beside a name no key pair of that id carries",
            vec![param("KeyPairId", &other_id), param("KeyName", "not-its-name")],
            "InvalidParameterCombination",
            format!("The key pair '{other_id}' is not named 'not-its-name'"),
        ),
    ];

    for (case, params, expected_code, expected_message) in rows {
        let (status, body) = delete(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DeleteKeyPair, got {status} with body \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: unexpected error code, body {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: unexpected error message, body {body}"
        );

        let held = described_ids(case, &router).await;
        assert_eq!(
            held.len(),
            2,
            "case {case:?}: a refused delete must remove nothing, got {held:?}"
        );
        assert!(
            held.contains(&held_id) && held.contains(&other_id),
            "case {case:?}: both held key pairs must survive a refused delete, got {held:?}"
        );
    }
}

/// RULE: `DryRun` "checks whether you have the required permissions for the action, without
/// actually making the request": a delete whose members are readable is answered
/// `DryRunOperation` and removes nothing, whether or not it names a key pair the account
/// holds, and whether or not a check that reads state would have refused the call — while a
/// value this operation cannot read as a key pair id is still refused on the request's
/// terms, there being nothing to check.
#[tokio::test]
async fn a_dry_run_delete_removes_nothing() {
    let router = emulator();

    let held_name = "held-key";
    let held_id = create("the held key pair", &router, held_name, "ed25519", &[]).await;

    // (case, the members beside DryRun)
    let dry_runs: Vec<(&str, Vec<Param>)> = vec![
        ("a held key pair by name", vec![param("KeyName", held_name)]),
        ("a held key pair by id", vec![param("KeyPairId", &held_id)]),
        (
            "a held key pair by id and name together",
            vec![param("KeyPairId", &held_id), param("KeyName", held_name)],
        ),
        (
            "a name the account never held",
            vec![param("KeyName", "never-created")],
        ),
        (
            "a well-formed id no key pair was issued",
            vec![param("KeyPairId", "key-0fedcba987654321f")],
        ),
        (
            // The flag is answered before the one check of this operation that reads state —
            // whether an id and a name given together name the same stored key pair — so a
            // pair that does NOT agree is answered the flag too, and only the real call made
            // without the flag is refused (which the refusal table drives).
            "an id beside a name the key pair of that id does not carry",
            vec![
                param("KeyPairId", &held_id),
                param("KeyName", "not-its-name"),
            ],
        ),
    ];

    for (case, mut params) in dry_runs {
        params.push(param("DryRun", "true"));
        let (status, body) = delete(&router, &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS} from a dry run, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_CODE,
            "case {case:?}: unexpected error code from a dry run, body {body}"
        );
        assert_eq!(
            error_message(&body),
            DRY_RUN_MESSAGE,
            "case {case:?}: unexpected message from a dry run, body {body}"
        );

        let held = described_ids(case, &router).await;
        assert_eq!(
            held,
            vec![held_id.clone()],
            "case {case:?}: a dry run must remove nothing, got {held:?}"
        );
    }

    // A value the operation cannot read as a key pair id is refused as such even under the
    // flag: there is nothing to look up, whatever the caller may do.
    let (status, body) = delete(
        &router,
        &[param("KeyPairId", "key-123"), param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 400,
        "a dry run naming an unreadable key pair id must be refused on the request's \
         terms, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a dry run naming an unreadable key pair id must be refused InvalidParameterValue, \
         body {body}"
    );

    // And the key pair is still there to delete for real, which is what the dry run said.
    let body = delete_ok(
        "the real delete after every dry run",
        &router,
        &[param("KeyName", held_name)],
    )
    .await;
    assert_eq!(
        member(&body, "keyPairId").as_deref(),
        Some(held_id.as_str()),
        "the real delete must report the id {held_id:?} the dry runs left in place, got \
         body {body}"
    );
    assert_gone(
        "the real delete after every dry run",
        &router,
        held_name,
        &held_id,
    )
    .await;
}
