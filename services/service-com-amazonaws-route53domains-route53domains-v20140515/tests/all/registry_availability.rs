// SPDX-License-Identifier: CC0-1.0

//! The registry's answer to `CheckDomainAvailability`, over awsJson1_1: WHICH names are
//! available, and the standard price the response carries for the ones that are.
//!
//! Two rules are pinned here:
//!
//! * `Availability` is the registry's answer, not this account's — a name ANY registrant
//!   holds is `UNAVAILABLE` (`DomainAvailability`: "The domain name is not available"),
//!   and only a well-formed, unheld name under a TLD the registrar offers is `AVAILABLE`
//!   ("You can register only domains designated as AVAILABLE");
//! * `Price` is present exactly when `Availability` is `AVAILABLE`
//!   (`CheckDomainAvailabilityResponse.Price`: "This element is returned only when
//!   Availability is AVAILABLE"), and carries the standard rate of the name's TLD with
//!   its required `Price` and `Currency` members and `PriceType` `STANDARD`.
//!
//! A name the registry cannot answer for at all is refused instead, and those refusals
//! are pinned here too: the TLD it does not delegate, a blank name, and the envelope
//! every constraint violation is reported in.
//!
//! Every test drives the real wire path through the shared harness in
//! `tests/all/domain_lifecycle.rs`.

use crate::domain_lifecycle::{error_code, person_contact, registration, Emulator};
use serde_json::{json, Value};

/// The standard yearly registration rates these cases expect, in USD. Source: the
/// per-TLD table `STANDARD_REGISTRATION_RATES` in
/// `src/state/registry.rs`, which carries Route 53's published registration price per
/// TLD; a TLD the table does not list takes `DEFAULT_STANDARD_REGISTRATION_RATE`
/// (20.00), which the `.shop` row below exercises.
const EXPECTED_RATES: &[(&str, f64)] = &[
    ("com", 16.00),
    ("net", 17.00),
    ("org", 16.00),
    ("io", 47.00),
    ("co.uk", 10.00),
    ("dev", 15.00),
    ("jp", 95.00),
    // Not in the table: the stated default rate.
    ("shop", 20.00),
];

/// `CheckDomainAvailability` for `name`, asserting a 200 and returning the body.
async fn check(emulator: &Emulator, name: &str) -> Value {
    let (status, body) = emulator
        .call("CheckDomainAvailability", &json!({ "DomainName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CheckDomainAvailability to answer 200, got {status} {body}"
    );
    body
}

/// Assert that `body` reports `name` as available at the standard rate `rate`.
fn assert_available_at(body: &Value, name: &str, rate: f64) {
    assert_eq!(
        body["Availability"].as_str(),
        Some("AVAILABLE"),
        "case {name:?}: an unheld name must be AVAILABLE, got {body}"
    );
    let price = &body["Price"];
    assert!(
        price.is_object(),
        "case {name:?}: Price is returned whenever Availability is AVAILABLE, got {body}"
    );
    assert_eq!(
        price["Price"].as_f64(),
        Some(rate),
        "case {name:?}: expected the TLD's standard rate {rate}, got {price}"
    );
    assert!(
        price["Price"].as_f64().is_some_and(|value| value >= 0.0),
        "case {name:?}: Price is @range(min: 0), got {price}"
    );
    assert_eq!(
        price["Currency"].as_str(),
        Some("USD"),
        "case {name:?}: expected the required Currency member, got {price}"
    );
    assert_eq!(
        price["PriceType"].as_str(),
        Some("STANDARD"),
        "case {name:?}: a regular TLD rate is PriceType STANDARD, got {price}"
    );
}

/// Assert that `body` reports `name` as unavailable and carries no price.
fn assert_unavailable(body: &Value, name: &str) {
    assert_eq!(
        body["Availability"].as_str(),
        Some("UNAVAILABLE"),
        "case {name:?}: a name the registry already holds must be UNAVAILABLE, got {body}"
    );
    assert!(
        body.get("Price").is_none_or(Value::is_null),
        "case {name:?}: Price is returned only when Availability is AVAILABLE, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The rule: an AVAILABLE name is priced at its TLD's standard rate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_available_name_carries_the_standard_price_of_its_tld() {
    // Rows differ along the TLD (generic, country-code, two-label suffix, a TLD the
    // price table does not list), the label's shape and the case of the request. Two
    // labels are checked under every TLD, because a standard rate belongs to the TLD
    // rather than to the name.
    let labels = ["sim-probe-free", "z0-9"];
    let emulator = Emulator::new();

    for (tld, rate) in EXPECTED_RATES {
        for label in labels {
            let name = format!("{label}.{tld}");
            let body = check(&emulator, &name).await;
            assert_available_at(&body, &name, *rate);

            // Domain names are not case sensitive, so the same name in another case is
            // the same answer at the same price.
            let upper = name.to_uppercase();
            let body = check(&emulator, &upper).await;
            assert_available_at(&body, &upper, *rate);
        }
    }
}

#[tokio::test]
async fn checking_a_name_twice_answers_the_same_thing() {
    // The operation is a pure read: nothing a caller sees may change because somebody
    // asked. Rows: an available name and one the registry holds.
    let emulator = Emulator::new();
    for name in ["sim-probe-repeat.com", "google.com"] {
        let first = check(&emulator, name).await;
        let second = check(&emulator, name).await;
        assert_eq!(
            first, second,
            "case {name:?}: a second check must answer exactly the first, got {first} then {second}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: Availability is the registry's answer, not this account's
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_held_by_another_registrant_is_unavailable() {
    // Rows differ along the TLD and the case of the request. Every one of these names is
    // registered in the real registries and none of them is held by this account, which
    // is exactly the case an account-scoped answer gets wrong.
    let held = [
        "google.com",
        "amazon.net",
        "example.org",
        "x.com",
        "iana.org",
        "amazon.co.uk",
        "GOOGLE.COM",
        "Example.Org",
    ];
    // Controls: free names under the same TLDs must still be available, so the fix
    // cannot be a blanket UNAVAILABLE.
    let free = [
        ("sim-probe-control.com", 16.00),
        ("sim-probe-control.net", 17.00),
        ("sim-probe-control.org", 16.00),
        ("sim-probe-control.co.uk", 10.00),
    ];

    let emulator = Emulator::new();
    for name in held {
        let body = check(&emulator, name).await;
        assert_unavailable(&body, name);
    }
    for (name, rate) in free {
        let body = check(&emulator, name).await;
        assert_available_at(&body, name, rate);
    }
}

#[tokio::test]
async fn a_name_held_by_another_registrant_is_neither_registrable_nor_readable_here() {
    // The answer has to be backed: a name reported UNAVAILABLE cannot then be registered,
    // and it is still not one of this account's domains. Rows differ along the TLD.
    let emulator = Emulator::new();
    for name in ["google.com", "example.org", "amazon.co.uk"] {
        let (status, body) = emulator
            .call("RegisterDomain", &registration(name, 1, &person_contact()))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {name:?}: a name the registry holds cannot be registered, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("Domain {name} is not available for registration").as_str()),
            "case {name:?}: the refusal must name the requested domain, got {body}"
        );

        let (status, body) = emulator
            .call("GetDomainDetail", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {name:?}: a third party's name is not this account's domain, got {status} {body}"
        );

        // The refused registration left no trace: the name is still the registry's.
        let body = check(&emulator, name).await;
        assert_unavailable(&body, name);
    }
}

// ---------------------------------------------------------------------------
// The rule: a TLD no registry delegates is refused, naming the TLD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unsupported_tld_is_refused_with_the_tld_in_the_message() {
    // The registry can only answer for a name under a TLD it delegates, so a name under
    // any other suffix is `UnsupportedTLD` — and the message's one variable part is the
    // TLD (the labels after the first), not the whole requested name. Rows differ along
    // the suffix: an unassigned label and two suffixes RFC 2606 reserves out of the
    // DNS.
    let cases = [
        ("sim-probe.doesnotexisttld", "doesnotexisttld"),
        ("sim-probe.invalid", "invalid"),
        ("sim-probe.test", "test"),
        // The same request in another case names the same TLD: domain names are not
        // case sensitive.
        ("SIM-PROBE.DoesNotExistTld", "doesnotexisttld"),
    ];
    let emulator = Emulator::new();

    for (name, tld) in cases {
        let (status, body) = emulator
            .call("CheckDomainAvailability", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "UnsupportedTLD".to_string()),
            "case {name:?}: expected 400 UnsupportedTLD, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The {tld} TLD is not supported").as_str()),
            "case {name:?}: the message must name the TLD alone, got {body}"
        );
    }

    // A third-level name is not a registrable name either, whatever a registry would
    // call its suffix, so only the refusal itself is asserted for it.
    let (status, body) = emulator
        .call(
            "CheckDomainAvailability",
            &json!({ "DomainName": "a.b.example.com" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "UnsupportedTLD".to_string()),
        "case \"a third-level name\": expected 400 UnsupportedTLD, got {status} {body}"
    );
}

#[tokio::test]
async fn a_blank_domain_name_is_refused_as_blank() {
    // A member that carries no name at all is a different problem from a name that
    // broke one of the syntax rules, and it is reported in its own words. Rows differ
    // along the form the blank value takes.
    let emulator = Emulator::new();
    for case in ["", " ", "   "] {
        let (status, body) = emulator
            .call("CheckDomainAvailability", &json!({ "DomainName": case }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("Invalid request. Problems: Domain name cannot be blank"),
            "case {case:?}: a blank name is refused as blank, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: the answer and the price follow the store, in both directions
// ---------------------------------------------------------------------------
#[tokio::test]
async fn registering_a_name_withdraws_its_price_and_deleting_it_restores_it() {
    // Rows differ along the TLD, so the price the answer carries differs per row.
    let cases = [
        ("sim-probe-lifecycle.com", 16.00),
        ("sim-probe-lifecycle.io", 47.00),
        ("sim-probe-lifecycle.co.uk", 10.00),
    ];
    let emulator = Emulator::new();

    for (name, rate) in cases {
        let body = check(&emulator, name).await;
        assert_available_at(&body, name, rate);

        let (status, body) = emulator
            .call("RegisterDomain", &registration(name, 1, &person_contact()))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected RegisterDomain to succeed, got {status} {body}"
        );

        // Read the registration back through the implemented reader: the name this
        // account now holds is the name that just became unavailable.
        let (status, detail) = emulator
            .call("GetDomainDetail", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            (status, detail["DomainName"].as_str()),
            (200, Some(name)),
            "case {name:?}: expected GetDomainDetail to report the registration, got {status} {detail}"
        );

        let body = check(&emulator, name).await;
        assert_unavailable(&body, name);

        let (status, body) = emulator
            .call("DeleteDomain", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected DeleteDomain to succeed, got {status} {body}"
        );

        let body = check(&emulator, name).await;
        assert_available_at(&body, name, rate);
    }
}

// ---------------------------------------------------------------------------
// The rule: a constraint violation is reported in the validation envelope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_constraint_violation_names_how_many_violations_were_found() {
    // The envelope is "<N> validation error(s) detected: " followed by the clause for
    // each member, so the count is the number of violations in THIS request. Rows
    // differ along the member and the constraint broken, and along HOW MANY members
    // break one: `DomainName` is @length(max: 255) and `DurationInYears` is
    // @range(min: 1, max: 10) (smithy_query shapes DomainName and DurationInYears).
    let past_bound = format!("{}.com", "a".repeat(252));
    let emulator = Emulator::new();

    // (case, operation, request, the clauses the envelope must carry)
    let cases: [(&str, &str, Value, Vec<String>); 3] = [
        (
            "one member past its length bound",
            "CheckDomainAvailability",
            json!({ "DomainName": past_bound }),
            vec![format!(
                "Value '{past_bound}' at 'domainName' failed to satisfy constraint: Member must \
                 have length less than or equal to 255"
            )],
        ),
        (
            "one member past its range bound",
            "RegisterDomain",
            registration("range.com", 11, &person_contact()),
            vec![
                "Value '11' at 'durationInYears' failed to satisfy constraint: Member must have \
                 value less than or equal to 10"
                    .to_string(),
            ],
        ),
        (
            "two members at once",
            "RegisterDomain",
            registration(&past_bound, 0, &person_contact()),
            vec![
                format!(
                    "Value '{past_bound}' at 'domainName' failed to satisfy constraint: Member \
                     must have length less than or equal to 255"
                ),
                "Value '0' at 'durationInYears' failed to satisfy constraint: Member must have \
                 value greater than or equal to 1"
                    .to_string(),
            ],
        ),
    ];

    for (case, operation, request, clauses) in cases {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        let found = clauses.len();
        let noun = if found == 1 { "error" } else { "errors" };
        let envelope = format!("{found} validation {noun} detected: ");
        assert!(
            message.starts_with(&envelope),
            "case {case:?}: expected the message to open with {envelope:?}, got {message:?}"
        );
        for clause in &clauses {
            assert!(
                message.contains(clause.as_str()),
                "case {case:?}: expected the message to carry {clause:?}, got {message:?}"
            );
        }
    }
}
