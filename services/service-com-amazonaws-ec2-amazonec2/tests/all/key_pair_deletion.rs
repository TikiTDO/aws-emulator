// SPDX-License-Identifier: CC0-1.0

//! What a `DeleteKeyPair` gives BACK to the store, read through the engine rather than over
//! the wire: the name the record held, the id it was stored under, and the share of the
//! store's caller-supplied byte budget it charged.
//!
//! The rule under test is that removing a key pair leaves the store exactly as it was before
//! the create: a key pair "stops being stored" by the one writer that also releases what it
//! charged, so a provision/teardown cycle repeated many times neither leaks budget nor leaves
//! a name or an id that still resolves. The wire answer of the delete itself — the id it
//! reports, the idempotent success for a key pair the account does not hold, and what it
//! refuses — is [`super::delete_key_pair`]'s subject; this suite is about the STORE, which a
//! wire-only suite cannot see.
//!
//! Each round of the cycle is a distinct input along what the rule generalises over: the key
//! type, the name's length, how many tags the record carries and how large they are, so no
//! round's charge is another's.

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements};
use super::wire::{param, Param};

/// The Region every call here is signed for. A key pair carries no Region member, so the
/// Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// One EC2 emulator with its engine, so the store's own counters can be read beside the wire.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The caller-supplied bytes the store currently holds, and how many key pairs it holds.
fn store_charge(engine: &engine::InMemoryEngine) -> (usize, usize) {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    (store.stored_bytes(), store.key_pairs.len())
}

/// One `CreateKeyPair` that must succeed, answering the id it minted.
async fn create(case: &str, router: &MultiServiceRouter, fields: &[Param]) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, "CreateKeyPair", fields).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateKeyPair, got {status} with body {body}"
    );
    element(&body, "keyPairId")
        .unwrap_or_else(|| panic!("case {case:?}: no keyPairId in {body}"))
}

/// One `DeleteKeyPair` by name that must succeed, answering the id it removed.
async fn delete_by_name(case: &str, router: &MultiServiceRouter, key_name: &str) -> Option<String> {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DeleteKeyPair",
        &[param("KeyName", key_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteKeyPair, got {status} with body {body}"
    );
    element(&body, "keyPairId")
}

/// Every key pair id the account holds, as `DescribeKeyPairs` reports them.
async fn held_ids(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeKeyPairs", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeKeyPairs, got {status} with body {body}"
    );
    elements(&body, "keyPairId")
}

/// One round of the cycle: what the create stores, so no two rounds charge the store alike.
struct Round {
    case: &'static str,
    /// `KeyType`, or `None` to take the documented default.
    key_type: Option<&'static str>,
    /// How many characters the name carries, inside the published maximum.
    name_length: usize,
    /// The tags the create assigns to the key pair.
    tags: &'static [(&'static str, &'static str)],
}

/// The rounds. The name length, the key type and the tag payload each differ, so each round's
/// record charges a different number of bytes.
const ROUNDS: &[Round] = &[
    Round {
        case: "a defaulted key pair with a short name and no tags",
        key_type: None,
        name_length: 8,
        tags: &[],
    },
    Round {
        case: "an ed25519 key pair with one tag",
        key_type: Some("ed25519"),
        name_length: 40,
        tags: &[("Owner", "TeamA")],
    },
    Round {
        case: "an rsa key pair with two tags and a long name",
        key_type: Some("rsa"),
        name_length: 200,
        tags: &[("Owner", "TeamB"), ("Env", "production-eu-west")],
    },
    Round {
        case: "a key pair at the published maximum name length",
        // "Up to 255 ASCII characters" (`CreateKeyPairRequest$KeyName`), AT the bound.
        key_type: Some("ed25519"),
        name_length: ec2::state::MAX_KEY_PAIR_NAME_CHARS,
        tags: &[],
    },
];

/// RULE: a create followed by a delete leaves the store holding exactly what it held before
/// the create — no key pair, none of its bytes, and neither its name nor its id resolving —
/// however many times the cycle is repeated and whatever the record held.
#[tokio::test]
async fn a_create_delete_cycle_gives_the_store_back_everything_it_charged() {
    let (router, engine) = emulator();
    let (empty_bytes, empty_count) = store_charge(&engine);
    assert_eq!(
        empty_count, 0,
        "a fresh account holds no key pair, got {empty_count}"
    );

    // A key pair that is created once and never deleted, so every assertion below is made
    // against a store that is NOT empty: a release that gave back too much would show up as a
    // charge below this one's.
    let kept_id = create(
        "the key pair that stays",
        &router,
        &[param("KeyName", "kept-across-the-cycle")],
    )
    .await;
    let (baseline_bytes, baseline_count) = store_charge(&engine);
    assert!(
        baseline_bytes > empty_bytes,
        "storing a key pair charges the store: got {baseline_bytes} after {empty_bytes}"
    );
    assert_eq!(
        baseline_count, 1,
        "the account holds the one key pair that stays, got {baseline_count}"
    );

    // Three passes over the rounds, so a leak of even one record's bytes per pass is visible
    // as a charge that never returns to the baseline.
    let mut removed_ids: Vec<String> = Vec::new();
    for pass in 1..=3usize {
        for round in ROUNDS {
            let case = round.case;
            let key_name = format!("{}-{pass}", "n".repeat(round.name_length));
            let key_name = key_name
                .char_indices()
                .take_while(|(index, _)| *index < ec2::state::MAX_KEY_PAIR_NAME_CHARS)
                .map(|(_, character)| character)
                .collect::<String>();

            let mut fields = vec![param("KeyName", &key_name)];
            if let Some(key_type) = round.key_type {
                fields.push(param("KeyType", key_type));
            }
            if !round.tags.is_empty() {
                fields.push(param("TagSpecification.1.ResourceType", "key-pair"));
                for (position, (key, value)) in (1..).zip(round.tags) {
                    fields.push(param(
                        &format!("TagSpecification.1.Tag.{position}.Key"),
                        key,
                    ));
                    fields.push(param(
                        &format!("TagSpecification.1.Tag.{position}.Value"),
                        value,
                    ));
                }
            }

            let key_pair_id = create(case, &router, &fields).await;
            let (held_bytes, held_count) = store_charge(&engine);
            assert!(
                held_bytes > baseline_bytes,
                "case {case:?} (pass {pass}): the record charges the store, got {held_bytes} \
                 against the baseline {baseline_bytes}"
            );
            assert_eq!(
                held_count,
                baseline_count + 1,
                "case {case:?} (pass {pass}): the account holds one more key pair, got \
                 {held_count}"
            );
            // An id the store minted is never one it minted before, within the cycle or
            // across it: the delete frees the NAME, not the id.
            assert!(
                !removed_ids.contains(&key_pair_id),
                "case {case:?} (pass {pass}): {key_pair_id} was already issued and deleted, \
                 got it again"
            );

            let reported = delete_by_name(case, &router, &key_name).await;
            assert_eq!(
                reported.as_deref(),
                Some(key_pair_id.as_str()),
                "case {case:?} (pass {pass}): the delete reports the record it removed, got \
                 {reported:?}"
            );
            removed_ids.push(key_pair_id);

            let (after_bytes, after_count) = store_charge(&engine);
            assert_eq!(
                after_bytes, baseline_bytes,
                "case {case:?} (pass {pass}): the delete gives back exactly what the create \
                 charged, got {after_bytes} against the baseline {baseline_bytes}"
            );
            assert_eq!(
                after_count, baseline_count,
                "case {case:?} (pass {pass}): only the key pair that stays is left, got \
                 {after_count}"
            );
            let held = held_ids(case, &router).await;
            assert_eq!(
                held,
                vec![kept_id.clone()],
                "case {case:?} (pass {pass}): the report holds only the key pair that stays, \
                 got {held:?}"
            );
        }
    }

    // The key pair that stays is still exactly as it was: nothing the cycle removed took any
    // of its bytes with it.
    let (final_bytes, final_count) = store_charge(&engine);
    assert_eq!(
        (final_bytes, final_count),
        (baseline_bytes, baseline_count),
        "the store is back to the one key pair that stays, got {final_bytes} byte(s) in \
         {final_count} record(s)"
    );
}
