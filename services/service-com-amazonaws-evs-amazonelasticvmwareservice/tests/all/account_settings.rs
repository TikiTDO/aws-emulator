// SPDX-License-Identifier: CC0-1.0

//! Account-level EVS settings, end to end over awsJson1_0.
//!
//! The rule these tests assert over a SET of inputs: account settings are a per-account,
//! per-REGION name -> value store that only `PutAccountSettings` writes — it creates or
//! overwrites exactly the names the request carries, refuses a name or a value outside
//! the domain `AccountSetting$name` documents, and leaves every other name at its
//! current value — and `GetAccountSettings` reports, for the Region the caller
//! addressed, every name that has been explicitly set and nothing else.
//!
//! Where each clause comes from (`smithy_query(query="operation",
//! operation="GetAccountSettings,PutAccountSettings")` and the docs it carries):
//!
//! * "Creates or updates account-level EVS settings … EVS settings included in the
//!   request are created or overwritten. Settings omitted from the request retain their
//!   current values." (`PutAccountSettings`.)
//! * "A list of regional account-level EVS settings, and their values, that were
//!   modified in this request." (`PutAccountSettingsResponse$settings`.)
//! * "Returns the configured EVS settings for your Amazon Web Services account in the
//!   specified Amazon Web Services Region. If no settings have been set, an empty list
//!   is returned." (`GetAccountSettings`.) `@readonly`, so a read changes nothing.
//! * "EVS settings that have never been explicitly set are omitted from the response."
//!   (`GetAccountSettingsResponse$settings`.)
//! * The DOMAIN, from `smithy_query(query="doc", shape="AccountSetting", member="name")`:
//!   "The name of the EVS setting. Valid values are: `vcfPortedCoreCount` (type: numeric
//!   string) - The total number of VCF license cores ported to Amazon EVS for the account
//!   in that Region. The maximum value is 1,000,000 cores." The modelled
//!   `@length(1..128)` and `@pattern ^[a-zA-Z0-9]+$` on `name` and `value` are the SHAPE
//!   of a setting, so every refusal row below is a request INSIDE that shape.
//!
//! The rows differ along what the rule generalises over: the magnitude and the spelling
//! of a value (its floor, the documented maximum, one past it, far past it, leading
//! zeros), whether a name is new or already held, a name repeated inside one call, the
//! number of settings in a call, how a name falls outside the documented set, and the
//! Region the caller addressed.
//!
//! A refusal states the SETTING it is about — `Setting '<name>' must not exceed
//! <maximum>.`, quoting the name the request gave and the maximum that setting is
//! documented with — and `tests/all/refusal_wording.rs` owns that wording rule across
//! the catalog and the request positions a setting can arrive in.

use serde_json::{json, Value};

use service_com_amazonaws_evs_amazonelasticvmwareservice::state as evs;

use crate::environment_lifecycle::{call, call_in_region, emulator, error_type, REGION};

/// The one setting name the model documents, and the maximum it documents for it:
/// "`vcfPortedCoreCount` (type: numeric string) … The maximum value is 1,000,000 cores."
/// Read from the catalog that doc sentence is recorded in, so this suite and the service
/// cannot drift apart over what the documented set is.
fn only_setting() -> &'static evs::AccountSettingSpec {
    assert_eq!(
        evs::ACCOUNT_SETTINGS.len(),
        1,
        "the model documents exactly one account setting; add rows below when it \
         documents another, got {:?}",
        evs::ACCOUNT_SETTINGS
    );
    &evs::ACCOUNT_SETTINGS[0]
}

/// One setting, as a row of the tables below spells it.
fn setting(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// A `PutAccountSettings` request body carrying `settings`.
fn put_body(settings: &[(String, String)]) -> Value {
    json!({
        "settings": settings
            .iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect::<Vec<Value>>(),
    })
}

/// The `settings` list a response carries, as (name, value) pairs in the order the
/// service reported them. Fails the test when the member is absent: both operations
/// report an empty list rather than omitting it.
fn reported(case: &str, operation: &str, body: &Value) -> Vec<(String, String)> {
    let list = body["settings"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: {operation} must report a `settings` list, got {body}")
    });
    list.iter()
        .map(|entry| {
            let name = entry["name"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: {operation} reported a setting with no name: {body}")
            });
            let value = entry["value"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: {operation} reported a setting with no value: {body}")
            });
            setting(name, value)
        })
        .collect()
}

/// One `PutAccountSettings` call this service accepts: what it answers, and what the
/// Region holds afterwards.
struct Step {
    /// What this row is a case of.
    case: &'static str,
    /// The settings the call carries, in request order.
    put: Vec<(String, String)>,
    /// The settings the call must report as modified, in order.
    modified: Vec<(String, String)>,
    /// Every setting the Region holds afterwards, by name — what `GetAccountSettings`
    /// must then report.
    held: Vec<(String, String)>,
}

#[tokio::test]
async fn a_put_writes_exactly_the_named_settings_and_a_get_reads_the_region_back() {
    let router = emulator();
    let spec = only_setting();
    let name = spec.name;

    // Nothing has been set yet, so the read is the empty list — not an absent member and
    // not a refusal.
    let (status, body) = call(&router, "GetAccountSettings", &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"an untouched Region\": expected 200, got {status} {body}"
    );
    assert!(
        reported("an untouched Region", "GetAccountSettings", &body).is_empty(),
        "case \"an untouched Region\": a Region with no settings reports the empty list, \
         got {body}"
    );

    let maximum = spec.maximum.to_string();
    let steps = vec![
        Step {
            case: "the first value ever set in this Region",
            put: vec![setting(name, "100")],
            modified: vec![setting(name, "100")],
            held: vec![setting(name, "100")],
        },
        Step {
            case: "overwriting it reports and holds the new value",
            put: vec![setting(name, "256")],
            modified: vec![setting(name, "256")],
            held: vec![setting(name, "256")],
        },
        Step {
            case: "a call that names no setting modifies nothing, so the value stands",
            put: Vec::new(),
            modified: Vec::new(),
            held: vec![setting(name, "256")],
        },
        Step {
            case: "the value at the documented maximum",
            put: vec![setting(name, &maximum)],
            modified: vec![setting(name, &maximum)],
            held: vec![setting(name, &maximum)],
        },
        Step {
            case: "a value of a single digit, the length floor",
            put: vec![setting(name, "0")],
            modified: vec![setting(name, "0")],
            held: vec![setting(name, "0")],
        },
        Step {
            case: "leading zeros are a numeric string inside the maximum",
            put: vec![setting(name, "0000042")],
            modified: vec![setting(name, "0000042")],
            held: vec![setting(name, "0000042")],
        },
        Step {
            case: "the same name twice in one call keeps the last value given",
            put: vec![setting(name, "1"), setting(name, "2")],
            modified: vec![setting(name, "2")],
            held: vec![setting(name, "2")],
        },
    ];

    for step in steps {
        let Step {
            case,
            put,
            modified,
            held,
        } = step;
        let body = put_body(&put);

        let (status, answer) = call(&router, "PutAccountSettings", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutAccountSettings to answer 200, got {status} {answer}"
        );
        let answered = reported(case, "PutAccountSettings", &answer);
        assert_eq!(
            answered, modified,
            "case {case:?}: expected the modified settings {modified:?}, got {answered:?}"
        );

        let (status, answer) = call(&router, "GetAccountSettings", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetAccountSettings to answer 200, got {status} {answer}"
        );
        let actual = reported(case, "GetAccountSettings", &answer);
        assert_eq!(
            actual, held,
            "case {case:?}: expected the Region to hold {held:?}, got {actual:?}"
        );

        // `PutAccountSettings` is `@idempotent`: the same call again reports the same
        // modification and leaves the same Region behind.
        let (status, answer) = call(&router, "PutAccountSettings", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?} repeated: expected 200, got {status} {answer}"
        );
        let repeated = reported(case, "PutAccountSettings", &answer);
        assert_eq!(
            repeated, modified,
            "case {case:?} repeated: expected the same modified settings {modified:?}, \
             got {repeated:?}"
        );
        let (_, answer) = call(&router, "GetAccountSettings", &json!({})).await;
        let actual = reported(case, "GetAccountSettings", &answer);
        assert_eq!(
            actual, held,
            "case {case:?} repeated: expected the Region to still hold {held:?}, got \
             {actual:?}"
        );
    }
}

/// One `PutAccountSettings` call this service refuses: the member path the refusal names,
/// the clauses its message states, and the settings the call carries.
struct Refused {
    /// What this row is a case of.
    case: &'static str,
    /// The member path the `fieldList` entry must name.
    member: String,
    /// The clauses the refusal's message must state, in order.
    clauses: Vec<String>,
    /// The settings the call carries, in request order.
    settings: Vec<(String, String)>,
}

/// What the top-level `message` of a refused request opens with, and what joins its
/// clauses when a request fails more than one check — the envelope Amazon EVS states this
/// refusal in: `Request validation failure: Setting '<name>' must not exceed <maximum>.`
const REFUSAL_PREFIX: &str = "Request validation failure: ";
/// See [`REFUSAL_PREFIX`].
const CLAUSE_JOINER: &str = " | ";

/// The clause Amazon EVS states for a value above the maximum a setting's own
/// documentation gives it: fixed text, with the setting named as the request named it and
/// the maximum that setting is documented with.
fn above_maximum(name: &str, maximum: u64) -> String {
    format!("Setting '{name}' must not exceed {maximum}.")
}

/// The clause for a value that is not of the TYPE the setting is documented with
/// (`vcfPortedCoreCount` is "type: numeric string"), stated about the same quoted setting.
fn not_the_documented_type(name: &str) -> String {
    format!("Setting '{name}' must be a numeric string.")
}

/// The clause for a name that is no documented setting at all, stated about the same
/// quoted setting and naming the settings there are.
fn not_a_setting(name: &str) -> String {
    format!(
        "Setting '{name}' is not a supported EVS setting: {}.",
        only_setting().name
    )
}

#[tokio::test]
async fn a_setting_outside_the_documented_domain_is_refused_and_stored_nowhere() {
    let router = emulator();
    let spec = only_setting();
    let name = spec.name;

    // One value inside the domain, so every row below is checked against a Region that
    // already holds a setting: a refusal must not disturb it either.
    const HELD: &str = "100";
    let (status, answer) = call(
        &router,
        "PutAccountSettings",
        &put_body(&[setting(name, HELD)]),
    )
    .await;
    assert_eq!(status, 200, "the accepted write failed: {status} {answer}");

    // Every row is INSIDE the modelled shape of a setting (1..128 characters matching
    // ^[a-zA-Z0-9]+$) and outside the documented domain, so each is refused by the
    // handler rather than by the boundary validator. (case, the member path the refusal
    // names, the settings the call carries.)
    let past_maximum = (spec.maximum + 1).to_string();
    let value_member = "settings.1.member.value".to_string();
    let name_member = "settings.1.member.name".to_string();
    let rows: Vec<Refused> = vec![
        Refused {
            case: "one core past the documented maximum",
            member: value_member.clone(),
            clauses: vec![above_maximum(name, spec.maximum)],
            settings: vec![setting(name, &past_maximum)],
        },
        Refused {
            case: "far past the documented maximum",
            member: value_member.clone(),
            clauses: vec![above_maximum(name, spec.maximum)],
            settings: vec![setting(name, "50000000")],
        },
        Refused {
            case: "a value with more digits than any integer type holds",
            member: value_member.clone(),
            clauses: vec![above_maximum(name, spec.maximum)],
            settings: vec![setting(name, &"9".repeat(128))],
        },
        Refused {
            case: "a value that is not a numeric string at all",
            member: value_member.clone(),
            clauses: vec![not_the_documented_type(name)],
            settings: vec![setting(name, "manycores")],
        },
        Refused {
            case: "a numeric string with a unit stuck to it",
            member: value_member.clone(),
            clauses: vec![not_the_documented_type(name)],
            settings: vec![setting(name, "100cores")],
        },
        Refused {
            case: "an exponent is not a numeric string either",
            member: value_member.clone(),
            clauses: vec![not_the_documented_type(name)],
            settings: vec![setting(name, "1e6")],
        },
        Refused {
            case: "a name the model does not document",
            member: name_member.clone(),
            clauses: vec![not_a_setting("vcfPortedSocketCount")],
            settings: vec![setting("vcfPortedSocketCount", "100")],
        },
        Refused {
            case: "the documented name with something appended",
            member: name_member.clone(),
            clauses: vec![not_a_setting(&format!("{name}X"))],
            settings: vec![setting(&format!("{name}X"), "100")],
        },
        Refused {
            case: "the documented name in another case",
            member: name_member,
            clauses: vec![not_a_setting(&name.to_uppercase())],
            settings: vec![setting(&name.to_uppercase(), "100")],
        },
        Refused {
            case: "a bad setting in the second position names that position",
            member: "settings.2.member.value".to_string(),
            clauses: vec![above_maximum(name, spec.maximum)],
            settings: vec![setting(name, "200"), setting(name, &past_maximum)],
        },
        Refused {
            case: "the first of two failures, ordered as the request named them",
            member: value_member,
            clauses: vec![
                not_the_documented_type(name),
                not_a_setting("notASetting"),
            ],
            settings: vec![setting(name, "manycores"), setting("notASetting", "1")],
        },
    ];

    for row in rows {
        let Refused {
            case,
            member,
            clauses,
            settings,
        } = row;
        let (status, answer) = call(&router, "PutAccountSettings", &put_body(&settings)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a setting outside the documented domain must be refused with \
             HTTP 400, got {status} {answer}"
        );
        assert_eq!(
            error_type(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        // The refusal states the SETTING, by the name the request gave it, and the
        // requirement its documentation gives it — one clause per failed setting, in
        // request order, and no wire member path.
        let expected = format!("{REFUSAL_PREFIX}{}", clauses.join(CLAUSE_JOINER));
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        let fields = answer["fieldList"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: the refusal must carry a fieldList, got {answer}")
        });
        assert_eq!(
            fields.first().and_then(|field| field["name"].as_str()),
            Some(member.as_str()),
            "case {case:?}: the first fieldList entry must name {member:?}, got {answer}"
        );

        // Refused, so nothing was written: the Region still holds exactly what it did.
        let (status, answer) = call(&router, "GetAccountSettings", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read-back, got {status} {answer}"
        );
        let actual = reported(case, "GetAccountSettings", &answer);
        let expected = vec![setting(name, HELD)];
        assert_eq!(
            actual, expected,
            "case {case:?}: a refused put stores nothing, expected {expected:?}, got \
             {actual:?}"
        );
    }
}

#[tokio::test]
async fn settings_are_held_per_region_and_a_region_reports_only_its_own() {
    let router = emulator();
    let name = only_setting().name;

    // The same setting name, set to a different value at four endpoints — including a
    // four-segment Region token — plus a Region left untouched.
    let rows = [
        (REGION, "100"),
        ("us-east-1", "200"),
        ("eu-central-1", "300"),
        ("us-gov-west-1", "400"),
    ];
    const UNTOUCHED: &str = "ap-southeast-4";

    for (region, value) in rows {
        let body = put_body(&[setting(name, value)]);
        let (status, answer) = call_in_region(&router, region, "PutAccountSettings", &body).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} {answer}"
        );
    }

    for (region, value) in rows {
        let (status, answer) =
            call_in_region(&router, region, "GetAccountSettings", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} {answer}"
        );
        let actual = reported(region, "GetAccountSettings", &answer);
        let expected = vec![setting(name, value)];
        assert_eq!(
            actual, expected,
            "case {region:?}: a Region reports only the settings set at its own \
             endpoint, expected {expected:?}, got {actual:?}"
        );
    }

    let (status, answer) =
        call_in_region(&router, UNTOUCHED, "GetAccountSettings", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {UNTOUCHED:?}: expected 200, got {status} {answer}"
    );
    let actual = reported(UNTOUCHED, "GetAccountSettings", &answer);
    assert!(
        actual.is_empty(),
        "case {UNTOUCHED:?}: a Region nobody wrote to holds no settings, got {actual:?}"
    );
}

#[tokio::test]
async fn reading_the_settings_leaves_them_exactly_as_a_later_caller_finds_them() {
    let router = emulator();
    let written = vec![setting(only_setting().name, "100")];

    let (status, answer) = call(&router, "PutAccountSettings", &put_body(&written)).await;
    assert_eq!(
        status, 200,
        "the write must be accepted, got {status} {answer}"
    );

    // `GetAccountSettings` is `@readonly`: five reads in a row report the same list, and
    // nothing in the store moves because it was read.
    for read in 1..=5 {
        let case = format!("read {read}");
        let (status, answer) = call(&router, "GetAccountSettings", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {answer}"
        );
        let actual = reported(&case, "GetAccountSettings", &answer);
        assert_eq!(
            actual, written,
            "case {case:?}: every read reports the same settings {written:?}, got \
             {actual:?}"
        );
    }
}

#[tokio::test]
async fn the_store_holds_settings_for_a_bounded_number_of_regions() {
    let router = emulator();
    let name = only_setting().name;

    // The Region a setting is written under comes from the request, so the store caps how
    // many Regions it holds settings for ([`evs::MAX_ACCOUNT_SETTING_REGIONS`]). Every
    // token below satisfies the Region grammar this service accepts.
    let regions: Vec<String> = (0..evs::MAX_ACCOUNT_SETTING_REGIONS)
        .map(|index| {
            let first = (b'a' + (index / 26) as u8) as char;
            let second = (b'a' + (index % 26) as u8) as char;
            format!("{first}{second}-west-1")
        })
        .collect();
    for region in &regions {
        let body = put_body(&[setting(name, "100")]);
        let (status, answer) = call_in_region(&router, region, "PutAccountSettings", &body).await;
        assert_eq!(
            status, 200,
            "case {region:?}: a Region inside the bound is accepted, got {status} {answer}"
        );
    }

    // One Region past the bound is refused, with this operation's modelled error.
    let body = put_body(&[setting(name, "100")]);
    let (status, answer) = call_in_region(&router, "zz-west-1", "PutAccountSettings", &body).await;
    assert_eq!(
        status, 400,
        "case \"one Region past the bound\": expected HTTP 400, got {status} {answer}"
    );
    assert_eq!(
        error_type(&answer),
        "ValidationException",
        "case \"one Region past the bound\": expected ValidationException, got {answer}"
    );

    // A Region the store already holds settings for is unaffected at the bound.
    let body = put_body(&[setting(name, "200")]);
    let (status, answer) =
        call_in_region(&router, &regions[0], "PutAccountSettings", &body).await;
    assert_eq!(
        status, 200,
        "case \"a held Region at the bound\": expected 200, got {status} {answer}"
    );
    let (_, answer) =
        call_in_region(&router, &regions[0], "GetAccountSettings", &json!({})).await;
    let actual = reported("a held Region at the bound", "GetAccountSettings", &answer);
    let expected = vec![setting(name, "200")];
    assert_eq!(
        actual, expected,
        "case \"a held Region at the bound\": expected {expected:?}, got {actual:?}"
    );
}
