// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two messages the profile-collection surface words itself.
//!
//! The rules under test:
//!
//! 1. The operations that address the PROFILE COLLECTION of a domain — `CreateProfile`
//!    (`POST /domains/{DomainName}/profiles`) and `SearchProfiles`
//!    (`POST /domains/{DomainName}/profiles/search`), per
//!    `smithy_query(query="operation", operation="CreateProfile,SearchProfiles")` — report
//!    a domain the service does not hold with `ResourceNotFoundException` (404) whose
//!    `Message` names the input MEMBER and the account the domain was looked for in; its
//!    variable parts are the `DomainName` the request carried and the emulator's own
//!    account id. The operations that address the DOMAIN resource word the same absence
//!    with their own sentence (naming the resource kind, with no account), so the two
//!    families are asserted side by side here.
//! 2. `DeleteProfile`'s success `Message` (`DeleteProfileResponse$Message`, documented as
//!    "A message that indicates the delete request is done") is ONE fixed value: it
//!    interpolates nothing from the request or from the store, so every successful delete
//!    answers byte-identically.
//!
//! Each rule is asserted over a SET of inputs — domain names spanning the modelled
//! `name` length and charset, and profiles of different content, including ones carrying
//! extra search keys — rather than for one recorded request. Every case goes through the
//! real router, so what is asserted is the wire behaviour a client sees. The HTTP
//! plumbing and the create helpers are this crate's one test harness, in
//! `tests/all/domain_profile_lifecycle.rs`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, error_message, Emulator};

/// A domain the store really holds, so a case can ask for a name that differs from it
/// only in case and still be an absence.
const HELD_DOMAIN: &str = "helddom";

/// The sentence the profile-collection operations word an absent domain with: the member
/// name, the requested `DomainName`, and the account the lookup ran in. The account comes
/// from the emulator's own account contract, never from a literal.
fn expected_collection_message(domain_name: &str) -> String {
    format!(
        "DomainName {domain_name} is not found in account {}",
        engine::emulator_account_id()
    )
}

/// Every profile-collection operation, asked for one domain name: the operation's name
/// and the `(status, body)` it answered.
async fn collection_lookups(emulator: &Emulator, domain: &str) -> Vec<(&'static str, u16, Value)> {
    let (create_status, create_body) = emulator
        .post(
            &format!("/domains/{domain}/profiles"),
            json!({ "AccountNumber": "A-1" }),
        )
        .await;
    let (search_status, search_body) = emulator
        .search(domain, json!({ "KeyName": "_account", "Values": ["A-1"] }), "")
        .await;
    vec![
        ("CreateProfile", create_status, create_body),
        ("SearchProfiles", search_status, search_body),
    ]
}

/// Rule 1, over a table of absent domain names whose rows differ along what the rule
/// generalises over: the name's length at both modelled bounds
/// (`CreateProfileRequest$DomainName` targets `name`, @length 1..64,
/// @pattern `^[a-zA-Z0-9_-]+$`), its charset, a name that differs from a held domain only
/// in case, and a name the store held until it was deleted.
#[tokio::test]
async fn an_absent_domain_is_refused_with_the_account_bearing_sentence() {
    let emulator = Emulator::new();
    emulator.create_domain(HELD_DOMAIN, 90).await;

    // A domain the store held and no longer holds: its absence is the same absence as a
    // name nothing ever created.
    emulator.create_domain("goneadom", 90).await;
    let (status, body) = emulator.send("DELETE", "/domains/goneadom", None).await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteDomain 200 for goneadom, got {status} {body:?}"
    );

    // (case, the DomainName the request carries)
    let rows: Vec<(&str, String)> = vec![
        ("the shortest name the model allows", "a".to_string()),
        ("the longest name the model allows", "d".repeat(64)),
        (
            "a name using the charset's underscore and hyphen",
            "my_absent-domain_7".to_string(),
        ),
        (
            "a name differing from a held domain only in case",
            HELD_DOMAIN.to_uppercase(),
        ),
        ("a name the store held until it was deleted", "goneadom".to_string()),
    ];

    for (case, domain) in &rows {
        for (operation, status, body) in collection_lookups(&emulator, domain).await {
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {case:?} on {operation}: expected 404 ResourceNotFoundException, \
                 got {status} {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                expected_collection_message(domain),
                "case {case:?} on {operation}: expected the account-bearing \
                 domain-not-found sentence, got {message:?}"
            );
            assert!(
                message.ends_with(engine::emulator_account_id())
                    && engine::emulator_account_id().len() == 12,
                "case {case:?} on {operation}: expected the refusal to end in the 12-digit \
                 account id, got {message:?}"
            );
        }

        // The refusals conjured nothing: the domain is still absent, and the operation
        // that addresses the DOMAIN resource words that absence its own way — no account
        // id, and the resource kind rather than the member name.
        let (status, read) = emulator.send("GET", &format!("/domains/{domain}"), None).await;
        let message = error_message(&read);
        assert_eq!(
            (status, message),
            (404, format!("Domain {domain} is not found").as_str()),
            "case {case:?} on GetDomain: expected the domain-resource wording for the \
             same absence, got {status} {read:?}"
        );
    }

    // The held domain is untouched by every refusal above.
    let (status, read) = emulator.send("GET", &format!("/domains/{HELD_DOMAIN}"), None).await;
    assert_eq!(
        (status, read.get("DomainName").and_then(Value::as_str)),
        (200, Some(HELD_DOMAIN)),
        "after the refusals: expected the held domain to still read back, got {status} {read:?}"
    );
}

/// One row of the delete-acknowledgement table: the profile a delete is asked for, the
/// domain it lives in, and the (`KeyName`, value) pairs associated with it after its
/// create.
struct Row {
    case: &'static str,
    domain: &'static str,
    profile: Value,
    keys: &'static [(&'static str, &'static str)],
}

/// Rule 2, over a table of profiles whose rows differ in what the profile carries —
/// nothing but one standard member, many standard members, attributes, and extra search
/// keys associated after the create — so any interpolation of the request or of the store
/// would show up as a differing body.
#[tokio::test]
async fn every_successful_delete_profile_answers_one_fixed_acknowledgement() {
    let emulator = Emulator::new();
    emulator.create_domain("ackdom", 90).await;
    emulator.create_domain("otherdom", 90).await;

    let rows: Vec<Row> = vec![
        Row {
            case: "a profile carrying one member",
            domain: "ackdom",
            profile: json!({ "FirstName": "Ada" }),
            keys: &[],
        },
        Row {
            case: "a profile carrying many standard members",
            domain: "ackdom",
            profile: json!({
                "AccountNumber": "A-77",
                "FirstName": "Grace",
                "LastName": "Hopper",
                "EmailAddress": "grace@example.invalid",
                "PhoneNumber": "+15550100",
            }),
            keys: &[],
        },
        Row {
            case: "a profile carrying attributes",
            domain: "ackdom",
            profile: json!({
                "FirstName": "Alan",
                "Attributes": { "tier": "gold", "segment": "eu" },
            }),
            keys: &[],
        },
        Row {
            case: "a profile carrying extra search keys",
            domain: "ackdom",
            profile: json!({ "FirstName": "Katherine" }),
            keys: &[("LoyaltyId", "L-1"), ("LoyaltyId", "L-2")],
        },
        Row {
            case: "a profile in another domain",
            domain: "otherdom",
            profile: json!({ "FirstName": "Edsger" }),
            keys: &[],
        },
    ];

    let mut acknowledgements: BTreeSet<String> = BTreeSet::new();
    for Row { case, domain, profile, keys } in &rows {
        let profile_id = emulator.create_profile(domain, profile.clone()).await;
        for (key_name, value) in keys.iter() {
            let (status, body) = emulator
                .post(
                    &format!("/domains/{domain}/profiles/keys"),
                    json!({ "ProfileId": &profile_id, "KeyName": key_name, "Values": [value] }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected AddProfileKey 200 for {key_name}, got {status} {body:?}"
            );
        }

        let (status, body) = emulator.delete_profile(domain, &profile_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteProfile 200, got {status} {body:?}"
        );
        let message = body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            message, "OK",
            "case {case:?}: expected the fixed delete acknowledgement, got {message:?}"
        );
        assert!(
            !message.contains(profile_id.as_str()) && !message.contains(domain),
            "case {case:?}: expected an acknowledgement interpolating neither the profile \
             id nor the domain, got {message:?}"
        );
        acknowledgements.insert(message);

        // The acknowledgement is backed by the delete: the profile is gone, so deleting
        // it again is the profile-not-found refusal, which is a DIFFERENT sentence.
        let (status, again) = emulator.delete_profile(domain, &profile_id).await;
        let message = error_message(&again);
        assert_eq!(
            (status, error_code(&again), message),
            (
                404,
                "ResourceNotFoundException",
                format!("Profile with profileId {profile_id} is not found in the domain").as_str()
            ),
            "case {case:?}: expected the second delete to report the profile absent, \
             got {status} {again:?}"
        );
    }

    assert_eq!(
        acknowledgements.len(),
        1,
        "expected every successful delete to answer the same acknowledgement, got \
         {acknowledgements:?}"
    );
}
