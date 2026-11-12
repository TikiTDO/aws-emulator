// SPDX-License-Identifier: CC0-1.0

//! How the conformance pack surface WORDS its refusal of a pack name this account and
//! Region holds no pack for.
//!
//! The rule pinned here: `NoSuchConformancePackException` carries the service's fixed
//! sentence "No such conformance pack exists. Conformance Pack(s): " followed by the names
//! THE REQUEST carried that resolved to no pack, in the order it carried them — rendered
//! inside square brackets by `DescribeConformancePacks`, the read whose refusal lists them
//! as the list they are, and bare by every operation that renders them without. The names
//! come from the request and from nothing else, so every operation of the surface words
//! the one refusal the one way.
//!
//! Rows span what the rule generalises over: the operation refusing, how the request names
//! the pack (one name, a list member, a single-pack member), and the form and length of the
//! name itself across the span `ConformancePackName` allows (`@length` 1..256, pattern
//! `^[a-zA-Z][-a-zA-Z0-9]*$`).
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` defines and the template helpers
//! `conformance_packs` defines.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::Emulator;
use crate::conformance_packs::{pack_from_body, past_the_deployment, rule_template};

/// The fixed text Config words `NoSuchConformancePackException` with, before the names the
/// request asked about. RECORDED wording.
const NO_SUCH_PACK_SENTENCE: &str = "No such conformance pack exists. Conformance Pack(s): ";

/// Pack names no pack exists for, spanning the form and length
/// `ConformancePackName` allows (`@length` 1..256, pattern `^[a-zA-Z][-a-zA-Z0-9]*$`): the
/// shortest a name can be, a hyphenated one, one carrying digits, one differing from a
/// stored pack's name only in case, and the longest a name can be.
fn absent_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest name the pattern allows", "z".to_string()),
        ("a hyphenated name", "ghost-pack".to_string()),
        ("a name carrying digits", "pack9000".to_string()),
        (
            "a name differing from a stored one only in case",
            "Alpha".to_string(),
        ),
        (
            "the longest name the length maximum allows",
            format!("g{}", "x".repeat(255)),
        ),
    ]
}

/// The operations that refuse a name no pack exists for, with how each request names the
/// pack and whether that operation renders the names inside square brackets.
///
/// `DescribeConformancePacks` is the read whose refusal lists the names as the list they
/// are; the others render the name bare.
fn refusing_reads(name: &str) -> Vec<(&'static str, Value, bool)> {
    vec![
        (
            "DescribeConformancePacks",
            json!({ "ConformancePackNames": [name] }),
            true,
        ),
        (
            "DescribeConformancePackCompliance",
            json!({ "ConformancePackName": name }),
            false,
        ),
        (
            "GetConformancePackComplianceDetails",
            json!({ "ConformancePackName": name }),
            false,
        ),
        (
            "GetConformancePackComplianceSummary",
            json!({ "ConformancePackNames": [name] }),
            false,
        ),
        (
            "DeleteConformancePack",
            json!({ "ConformancePackName": name }),
            false,
        ),
    ]
}

/// The status, error code and message of a refused call.
async fn refusal(emulator: &Emulator, operation: &str, body: &Value) -> (u16, String, String) {
    let (status, parsed) = emulator.call(operation, body).await;
    let code = parsed["__type"]
        .as_str()
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("<no __type in {parsed}>"));
    let message = parsed["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected the refusal to carry a message, got {parsed}"))
        .to_string();
    (status, code, message)
}

#[tokio::test]
async fn every_operation_words_the_no_such_pack_refusal_the_same_way() {
    let emulator = Emulator::new();
    // One pack this account DOES hold, so the refusals below are about the name asked
    // for rather than about an empty account.
    emulator
        .ok(
            "PutConformancePack",
            &pack_from_body("alpha", &rule_template("only-rule")),
        )
        .await;
    tokio::time::sleep(past_the_deployment()).await;

    for (name_case, name) in absent_names() {
        for (operation, body, bracketed) in refusing_reads(&name) {
            let case = format!("{operation} with {name_case}");
            let (status, code, message) = refusal(&emulator, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NoSuchConformancePackException"),
                "case {case:?}: expected 400 NoSuchConformancePackException, got {status} {code} \
                 with message {message:?}"
            );
            let expected = if bracketed {
                format!("{NO_SUCH_PACK_SENTENCE}[{name}]")
            } else {
                format!("{NO_SUCH_PACK_SENTENCE}{name}")
            };
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }
    }

    // The pack this account holds is still readable: the refusals refused their requests,
    // they did not change what the account holds.
    let described = emulator.ok("DescribeConformancePacks", &json!({})).await;
    let names: Vec<&str> = described["ConformancePackDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("expected ConformancePackDetails, got {described}"))
        .iter()
        .map(|detail| detail["ConformancePackName"].as_str().unwrap_or("<none>"))
        .collect();
    assert_eq!(
        names,
        vec!["alpha"],
        "the refusals must leave the stored packs as they were, got {described}"
    );
}

#[tokio::test]
async fn the_refusal_names_every_name_the_request_carried_that_resolves_to_no_pack() {
    // Rows differ along WHICH of the names a list request carried resolve to no pack, and
    // in which order it carried them — the refusal names those, as the request ordered
    // them, and names no pack that does exist.
    let cases: [(&str, &[&str], &str); 4] = [
        ("one name beside a stored one", &["alpha", "ghost"], "ghost"),
        ("two names no pack exists for", &["ghost", "phantom"], "ghost, phantom"),
        (
            "the same two names in the other order",
            &["phantom", "ghost"],
            "phantom, ghost",
        ),
        (
            "an absent name between two stored ones",
            &["alpha", "ghost", "bravo"],
            "ghost",
        ),
    ];

    let emulator = Emulator::new();
    for stored in ["alpha", "bravo"] {
        emulator
            .ok(
                "PutConformancePack",
                &pack_from_body(stored, &rule_template("only-rule")),
            )
            .await;
        tokio::time::sleep(past_the_deployment()).await;
    }

    for (case, names, missing) in cases {
        // The list read renders them inside square brackets ...
        let (status, code, message) = refusal(
            &emulator,
            "DescribeConformancePacks",
            &json!({ "ConformancePackNames": names }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConformancePackException"),
            "case {case:?}: expected 400 NoSuchConformancePackException, got {status} {code} \
             with message {message:?}"
        );
        let expected = format!("{NO_SUCH_PACK_SENTENCE}[{missing}]");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        // ... and the summary read, which takes the same list of names, renders them bare.
        let (status, code, message) = refusal(
            &emulator,
            "GetConformancePackComplianceSummary",
            &json!({ "ConformancePackNames": names }),
        )
        .await;
        let expected = format!("{NO_SUCH_PACK_SENTENCE}{missing}");
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "NoSuchConformancePackException", expected.as_str()),
            "case {case:?}: expected 400 NoSuchConformancePackException with {expected:?}, got \
             {status} {code} with message {message:?}"
        );
    }
}
