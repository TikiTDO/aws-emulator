// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `AddProfileKey`.
//!
//! The rule under test: associating a key value with a profile persists the
//! (`KeyName`, value) pair on that profile and indexes it in its domain, so
//! `SearchProfiles` finds the profile through the pair and reports it in `FoundByItems`;
//! the pair is added to the ones the profile already carries (a repeat associates nothing
//! further, and no association replaces another), it survives a write of the profile's
//! standard members, it moves with the profile on a merge, and it goes when the profile
//! goes. A request naming a domain or a profile the store does not hold is
//! `ResourceNotFoundException` (404) and writes nothing.
//!
//! Each test asserts that over a SET of inputs — key names and values of different
//! shapes, counts and sizes — rather than for one recorded request. Every case goes
//! through the real router, so what is asserted is the wire behaviour a client sees. The
//! HTTP plumbing and the create helpers are this crate's one test harness, in
//! `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use service_com_amazonaws_customerprofiles_customerprofiles_20200815::state::MAX_KEY_VALUES_PER_PROFILE;

use crate::domain_profile_lifecycle::{error_code, item_ids, Emulator};

/// `AddProfileKey` over the wire: `POST /domains/{DomainName}/profiles/keys`.
async fn add_key(
    emulator: &Emulator,
    domain: &str,
    profile_id: &str,
    key_name: &str,
    values: &[&str],
) -> (u16, Value) {
    emulator
        .post(
            &format!("/domains/{domain}/profiles/keys"),
            json!({
                "ProfileId": profile_id,
                "KeyName": key_name,
                "Values": values,
            }),
        )
        .await
}

/// The `Message` an error body carries, whichever casing the protocol rendered it in.
fn error_message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The `SearchProfiles` response for one (key name, value) pair, asserted to have
/// answered 200.
async fn search(emulator: &Emulator, domain: &str, key_name: &str, value: &str) -> Value {
    let (status, body) = emulator
        .search(
            domain,
            json!({ "KeyName": key_name, "Values": [value] }),
            "",
        )
        .await;
    assert_eq!(
        status, 200,
        "case {key_name:?}={value:?}: expected SearchProfiles 200, got {status} {body:?}"
    );
    body
}

/// The ids `SearchProfiles` finds for one (key name, value) pair.
async fn found_ids(emulator: &Emulator, domain: &str, key_name: &str, value: &str) -> Vec<String> {
    item_ids(&search(emulator, domain, key_name, value).await)
}

/// The `FoundByItems` the search reported for `profile_id`, as (key name, values) pairs.
async fn found_by(
    emulator: &Emulator,
    domain: &str,
    key_name: &str,
    value: &str,
    profile_id: &str,
) -> Vec<(String, Vec<String>)> {
    let body = search(emulator, domain, key_name, value).await;
    body.get("Items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("ProfileId").and_then(Value::as_str) == Some(profile_id))
        .flat_map(|item| {
            item.get("FoundByItems")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .map(|found| {
            (
                found
                    .get("KeyName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                found
                    .get("Values")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| value.as_str().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// The `Values` an `AddProfileKey` response reported.
fn response_values(body: &Value) -> Vec<String> {
    body.get("Values")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A `ProfileId` that satisfies the modelled pattern `^[a-f0-9]{32}$` but that no create
/// in these tests minted, so the store cannot hold it.
fn unknown_id(seed: u64) -> String {
    format!("{:032x}", 0xdcdc_0000_0000_0000u64 + seed)
}

/// Every shape of key a request may carry, each paired with the values associated under
/// it. The key names are the predefined ones `AddProfileKey$KeyName` lists (`_orderId`,
/// `_ctrContactId`, `_zendeskTicketId`) and names of a caller's own, at both ends of the
/// member's modelled length (1..64) and over the characters its pattern
/// `^[a-zA-Z0-9_-]+$` admits; the values span one value and several, and both ends of
/// `requestValueList`'s member length (1..255).
fn key_cases() -> Vec<(&'static str, String, Vec<String>)> {
    vec![
        (
            "one value under a predefined contact-record key",
            "_ctrContactId".to_string(),
            vec!["11111111-2222-3333-4444-555555555555".to_string()],
        ),
        (
            "three values under one predefined key",
            "_orderId".to_string(),
            vec!["ord-1".to_string(), "ord-2".to_string(), "ord-3".to_string()],
        ),
        (
            "a predefined connector key the issue did not cite",
            "_zendeskTicketId".to_string(),
            vec!["42".to_string()],
        ),
        (
            "a caller's own key name",
            "LoyaltyId".to_string(),
            vec!["L-9".to_string()],
        ),
        (
            "key name and value of one character each (both minimum lengths)",
            "k".to_string(),
            vec!["v".to_string()],
        ),
        (
            "key name at the maximum modelled length of 64",
            "k".repeat(64),
            vec!["long-key-name".to_string()],
        ),
        (
            "value at the maximum modelled length of 255",
            "crm-system_7".to_string(),
            vec!["v".repeat(255)],
        ),
        (
            "every character class the key-name pattern admits",
            "Mixed-Key_09".to_string(),
            vec!["mixed".to_string()],
        ),
    ]
}

/// The rule itself: whatever the shape of the key and its values, the pair is persisted
/// and the profile is findable through it, with the key reported in `FoundByItems`.
#[tokio::test]
async fn an_added_key_makes_the_profile_findable_by_every_value_it_carries() {
    let emulator = Emulator::new();
    emulator.create_domain("keysdom", 30).await;

    for (case, key_name, values) in key_cases() {
        // One profile per case, so a search by the case's key answers about that case
        // alone. Its own standard keys are not what the case is about.
        let profile_id = emulator
            .create_profile("keysdom", json!({ "AccountNumber": format!("acct-{case}") }))
            .await;

        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let (status, body) = add_key(&emulator, "keysdom", &profile_id, &key_name, &borrowed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected AddProfileKey 200, got {status} {body:?}"
        );
        assert_eq!(
            body.get("KeyName").and_then(Value::as_str),
            Some(key_name.as_str()),
            "case {case:?}: expected the KeyName it was given, got {:?}",
            body.get("KeyName")
        );
        assert_eq!(
            response_values(&body),
            values,
            "case {case:?}: expected the Values it associated, got {:?}",
            body.get("Values")
        );

        for value in &values {
            assert_eq!(
                found_ids(&emulator, "keysdom", &key_name, value).await,
                vec![profile_id.clone()],
                "case {case:?}: expected SearchProfiles by {key_name:?}={value:?} to find \
                 only {profile_id}, got {:?}",
                found_ids(&emulator, "keysdom", &key_name, value).await
            );
            assert_eq!(
                found_by(&emulator, "keysdom", &key_name, value, &profile_id).await,
                vec![(key_name.clone(), vec![value.clone()])],
                "case {case:?}: expected FoundByItems to report {key_name:?}={value:?}, got {:?}",
                found_by(&emulator, "keysdom", &key_name, value, &profile_id).await
            );
        }

        // A key associated with one profile says nothing about another: the second
        // profile is not reachable through it.
        let other = emulator
            .create_profile("keysdom", json!({ "AccountNumber": format!("other-{case}") }))
            .await;
        for value in &values {
            let found = found_ids(&emulator, "keysdom", &key_name, value).await;
            assert!(
                !found.contains(&other),
                "case {case:?}: expected {key_name:?}={value:?} not to reach the unrelated \
                 profile {other}, got {found:?}"
            );
        }
    }
}

/// Values accumulate: each call adds to what the profile already carries, a repeat adds
/// nothing, and two calls that race leave both associations in place.
#[tokio::test]
async fn associating_a_value_never_drops_one_already_associated() {
    let emulator = Emulator::new();
    emulator.create_domain("accdom", 30).await;
    let profile_id = emulator
        .create_profile("accdom", json!({ "FirstName": "Grace" }))
        .await;

    // Each row is one call; after it, every pair listed so far must still find the
    // profile exactly once. The rows differ in the key they write, the number of values,
    // and whether a value repeats one already associated.
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        ("first value of a key", "_ctrContactId", vec!["ctr-1"]),
        ("second value of the same key", "_ctrContactId", vec!["ctr-2"]),
        (
            "two more values of that key at once",
            "_ctrContactId",
            vec!["ctr-3", "ctr-4"],
        ),
        (
            "a value already associated, repeated",
            "_ctrContactId",
            vec!["ctr-1"],
        ),
        ("a value repeated within one request", "_orderId", vec!["ord-1", "ord-1"]),
        ("a different key", "LoyaltyId", vec!["L-1"]),
    ];
    let mut associated: Vec<(String, String)> = Vec::new();
    for (case, key_name, values) in rows {
        let (status, body) = add_key(&emulator, "accdom", &profile_id, key_name, &values).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected AddProfileKey 200, got {status} {body:?}"
        );
        // The values this call associated, deduplicated: a key carries a SET of values.
        let mut expected: Vec<String> = Vec::new();
        for value in &values {
            if !expected.iter().any(|kept| kept == value) {
                expected.push((*value).to_string());
            }
        }
        assert_eq!(
            response_values(&body),
            expected,
            "case {case:?}: expected the values it associated, got {:?}",
            body.get("Values")
        );

        for value in &values {
            let pair = (key_name.to_string(), (*value).to_string());
            if !associated.contains(&pair) {
                associated.push(pair);
            }
        }
        for (key, value) in &associated {
            assert_eq!(
                found_ids(&emulator, "accdom", key, value).await,
                vec![profile_id.clone()],
                "case {case:?}: expected {key:?}={value:?} to still find {profile_id} exactly \
                 once, got {:?}",
                found_ids(&emulator, "accdom", key, value).await
            );
        }
    }

    // Two associations made without either having observed the other: both land.
    let (raced_left, raced_right) = tokio::join!(
        add_key(&emulator, "accdom", &profile_id, "RaceKey", &["left"]),
        add_key(&emulator, "accdom", &profile_id, "RaceKey", &["right"]),
    );
    for (case, (status, body)) in [("left", raced_left), ("right", raced_right)] {
        assert_eq!(
            status, 200,
            "case {case:?}: expected AddProfileKey 200, got {status} {body:?}"
        );
    }
    for value in ["left", "right"] {
        assert_eq!(
            found_ids(&emulator, "accdom", "RaceKey", value).await,
            vec![profile_id.clone()],
            "case {value:?}: expected both raced associations to survive, got {:?}",
            found_ids(&emulator, "accdom", "RaceKey", value).await
        );
    }
}

/// A key belongs to the profile: a write of the profile's standard members keeps it, a
/// merge moves it onto the profile that absorbed its own, and deleting the profile takes
/// it with the profile.
#[tokio::test]
async fn an_added_key_follows_the_profile_it_was_associated_with() {
    let emulator = Emulator::new();
    emulator.create_domain("lifedom", 30).await;

    // A write of the profile's standard members re-indexes it; the key it carries is not
    // part of that write, so it is still there afterwards.
    let updated = emulator
        .create_profile(
            "lifedom",
            json!({ "FirstName": "Ada", "EmailAddress": "ada@example.com" }),
        )
        .await;
    let (status, body) = add_key(&emulator, "lifedom", &updated, "_orderId", &["ord-7"]).await;
    assert_eq!(status, 200, "AddProfileKey: expected 200, got {status} {body:?}");
    let (status, body) = emulator
        .send(
            "PUT",
            "/domains/lifedom/profiles",
            Some(json!({ "ProfileId": updated, "EmailAddress": "ada@elsewhere.example" })),
        )
        .await;
    assert_eq!(status, 200, "UpdateProfile: expected 200, got {status} {body:?}");
    assert_eq!(
        found_ids(&emulator, "lifedom", "_orderId", "ord-7").await,
        vec![updated.clone()],
        "expected the key to survive an update of the profile's standard members, got {:?}",
        found_ids(&emulator, "lifedom", "_orderId", "ord-7").await
    );
    assert_eq!(
        found_ids(&emulator, "lifedom", "_email", "ada@elsewhere.example").await,
        vec![updated.clone()],
        "expected the update's own value to be indexed too, got {:?}",
        found_ids(&emulator, "lifedom", "_email", "ada@elsewhere.example").await
    );

    // "All the profileKeys in the ProfileIdsToBeMerged will be moved to the main
    // profile", so the key reaches the main profile and nothing else.
    let main = emulator
        .create_profile("lifedom", json!({ "FirstName": "Main" }))
        .await;
    let absorbed = emulator
        .create_profile("lifedom", json!({ "FirstName": "Absorbed" }))
        .await;
    let (status, body) = add_key(&emulator, "lifedom", &absorbed, "LoyaltyId", &["L-5"]).await;
    assert_eq!(status, 200, "AddProfileKey: expected 200, got {status} {body:?}");
    let (status, body) = emulator
        .post(
            "/domains/lifedom/profiles/objects/merge",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": [absorbed] }),
        )
        .await;
    assert_eq!(status, 200, "MergeProfiles: expected 200, got {status} {body:?}");
    assert_eq!(
        found_ids(&emulator, "lifedom", "LoyaltyId", "L-5").await,
        vec![main.clone()],
        "expected the merged profile's key to be moved onto the main profile, got {:?}",
        found_ids(&emulator, "lifedom", "LoyaltyId", "L-5").await
    );

    // A deleted profile is not reachable through the keys it carried.
    let deleted = emulator
        .create_profile("lifedom", json!({ "FirstName": "Gone" }))
        .await;
    let (status, body) = add_key(&emulator, "lifedom", &deleted, "LoyaltyId", &["L-6"]).await;
    assert_eq!(status, 200, "AddProfileKey: expected 200, got {status} {body:?}");
    let (status, body) = emulator.delete_profile("lifedom", &deleted).await;
    assert_eq!(status, 200, "DeleteProfile: expected 200, got {status} {body:?}");
    assert!(
        found_ids(&emulator, "lifedom", "LoyaltyId", "L-6")
            .await
            .is_empty(),
        "expected the deleted profile's key to be gone, got {:?}",
        found_ids(&emulator, "lifedom", "LoyaltyId", "L-6").await
    );
    // The key the merge moved is unaffected by an unrelated delete.
    assert_eq!(
        found_ids(&emulator, "lifedom", "LoyaltyId", "L-5").await,
        vec![main],
        "expected the merged key to be untouched by an unrelated delete, got {:?}",
        found_ids(&emulator, "lifedom", "LoyaltyId", "L-5").await
    );
}

/// A request naming something the store does not hold is refused, and refusing writes
/// nothing: the pair it carried reaches no profile afterwards.
#[tokio::test]
async fn a_key_for_an_absent_domain_or_profile_is_not_found() {
    let emulator = Emulator::new();
    emulator.create_domain("refusedom", 30).await;
    emulator.create_domain("otherdom", 30).await;
    let here = emulator
        .create_profile("refusedom", json!({ "FirstName": "Here" }))
        .await;
    let elsewhere = emulator
        .create_profile("otherdom", json!({ "FirstName": "Elsewhere" }))
        .await;

    let absent = unknown_id(1);
    let rows: Vec<(&str, &str, String, String)> = vec![
        (
            "a domain the store does not hold",
            "nosuchdom",
            here.clone(),
            "Domain nosuchdom is not found".to_string(),
        ),
        (
            "a profile id no create minted",
            "refusedom",
            absent.clone(),
            format!("Profile with profileId {absent} is not found in the domain"),
        ),
        (
            "a profile another domain holds",
            "refusedom",
            elsewhere.clone(),
            format!("Profile with profileId {elsewhere} is not found in the domain"),
        ),
    ];
    for (case, domain, profile_id, message) in rows {
        let (status, body) = add_key(&emulator, domain, &profile_id, "RefusedKey", &["r-1"]).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {body:?}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the service's wording, got {:?}",
            error_message(&body)
        );
    }
    for (case, domain) in [("refusedom", "refusedom"), ("otherdom", "otherdom")] {
        assert!(
            found_ids(&emulator, domain, "RefusedKey", "r-1")
                .await
                .is_empty(),
            "case {case:?}: expected a refused request to have written nothing, got {:?}",
            found_ids(&emulator, domain, "RefusedKey", "r-1").await
        );
    }
}

/// The store's cap on how many key values one profile carries: at the cap a request is
/// served, past it refused with nothing written, and a value already associated never
/// counts as growth.
#[tokio::test]
async fn a_profile_carries_at_most_the_capped_number_of_key_values() {
    let emulator = Emulator::new();
    emulator.create_domain("capdom", 30).await;

    // One request past the cap: refused, and the values it carried reach no profile.
    let refused = emulator
        .create_profile("capdom", json!({ "FirstName": "Refused" }))
        .await;
    let too_many: Vec<String> = (0..=MAX_KEY_VALUES_PER_PROFILE)
        .map(|index| format!("v-{index}"))
        .collect();
    let borrowed: Vec<&str> = too_many.iter().map(String::as_str).collect();
    let (status, body) = add_key(&emulator, "capdom", &refused, "CapKey", &borrowed).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {:?} values: expected 400 BadRequestException, got {status} {body:?}",
        too_many.len()
    );
    assert_eq!(
        error_message(&body),
        format!(
            "The maximum number of profile keys for profile {refused} has been reached: {MAX_KEY_VALUES_PER_PROFILE}"
        ),
        "case over the cap: expected the refusal to name the profile and the limit, got {:?}",
        error_message(&body)
    );
    assert!(
        found_ids(&emulator, "capdom", "CapKey", "v-0")
            .await
            .is_empty(),
        "case over the cap: expected nothing written, got {:?}",
        found_ids(&emulator, "capdom", "CapKey", "v-0").await
    );

    // Exactly the cap: served, and every value it carried is findable.
    let filled = emulator
        .create_profile("capdom", json!({ "FirstName": "Filled" }))
        .await;
    let at_cap: Vec<&str> = borrowed[..MAX_KEY_VALUES_PER_PROFILE].to_vec();
    let (status, body) = add_key(&emulator, "capdom", &filled, "CapKey", &at_cap).await;
    assert_eq!(
        status, 200,
        "case at the cap: expected AddProfileKey 200, got {status} {body:?}"
    );
    for value in ["v-0", "v-500", "v-999"] {
        assert_eq!(
            found_ids(&emulator, "capdom", "CapKey", value).await,
            vec![filled.clone()],
            "case at the cap: expected {value:?} to find {filled}, got {:?}",
            found_ids(&emulator, "capdom", "CapKey", value).await
        );
    }

    // A value the profile already carries is not growth, so it is still served at the
    // cap; one more fresh value is one too many.
    let (status, body) = add_key(&emulator, "capdom", &filled, "CapKey", &["v-0"]).await;
    assert_eq!(
        status, 200,
        "case a repeat at the cap: expected AddProfileKey 200, got {status} {body:?}"
    );
    let (status, body) = add_key(&emulator, "capdom", &filled, "CapKey", &["one-too-many"]).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case one past the cap: expected 400 BadRequestException, got {status} {body:?}"
    );
    assert!(
        found_ids(&emulator, "capdom", "CapKey", "one-too-many")
            .await
            .is_empty(),
        "case one past the cap: expected nothing written, got {:?}",
        found_ids(&emulator, "capdom", "CapKey", "one-too-many").await
    );
}
