// SPDX-License-Identifier: CC0-1.0

//! EMR block public access, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, so these are the same bytes an SDK client sends. Nothing
//! reaches into the service's state struct: a configuration is only ever read back
//! through `GetBlockPublicAccessConfiguration`, which is what makes these tests
//! evidence that the response is computed from persisted state.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// The wire harness every suite in this target shares, rather than a copy per suite.
use crate::job_flow_lifecycle::{at, Emr};

/// `com.amazonaws.emr#Port`'s published bounds — `PortRange$MinRange` and
/// `PortRange$MaxRange` both target `Port`, `range { min: -1, max: 65535 }`
/// (`smithy_query(query="shape", shape="PortRange")`).
const PORT_MIN: i64 = -1;
const PORT_MAX: i64 = 65535;

/// The port the account default permits: "By default, Port 22, which is used for SSH
/// access to the cluster Amazon EC2 instances, is in the list of
/// `PermittedPublicSecurityGroupRuleRanges`."
/// (`BlockPublicAccessConfiguration$PermittedPublicSecurityGroupRuleRanges` doc.)
const DEFAULT_PERMITTED_PORT: i64 = 22;

/// `PutBlockPublicAccessConfiguration` with `configuration`, which must be accepted.
async fn put(emr: &Emr, case: &str, configuration: &Value) {
    let (status, payload) = emr
        .call(
            "PutBlockPublicAccessConfiguration",
            json!({ "BlockPublicAccessConfiguration": configuration }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutBlockPublicAccessConfiguration must answer 200, \
         got {status} with {payload}"
    );
}

/// The whole `GetBlockPublicAccessConfiguration` output.
async fn get(emr: &Emr, case: &str) -> Value {
    let (status, payload) = emr
        .call("GetBlockPublicAccessConfiguration", json!({}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetBlockPublicAccessConfiguration must answer 200, \
         got {status} with {payload}"
    );
    payload
}

fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// The configuration the response reports, and the metadata beside it.
fn parts(output: &Value) -> (&Value, &Value) {
    (
        at(output, &["BlockPublicAccessConfiguration"]),
        at(output, &["BlockPublicAccessConfigurationMetadata"]),
    )
}

/// The configuration a put of `configuration` leaves stored: what was put, with the
/// exception list it omitted canonicalized to the empty list — "no public-port
/// exception" has ONE representation, so the omitted-member and the explicit-`[]` cases
/// read back the same.
fn canonical(configuration: &Value) -> Value {
    let mut canonical = configuration.clone();
    if canonical.get("PermittedPublicSecurityGroupRuleRanges").is_none() {
        canonical["PermittedPublicSecurityGroupRuleRanges"] = json!([]);
    }
    canonical
}

/// `BlockPublicAccessConfigurationMetadata` names the principal `created_by` — an IAM
/// ARN, or the empty string for a configuration no principal has written — and an instant
/// no later than `at_most`.
fn assert_metadata(
    case: &str,
    metadata: &Value,
    created_by: CreatedBy,
    at_least: f64,
    at_most: f64,
) {
    let arn = at(metadata, &["CreatedByArn"]).as_str().unwrap_or_default();
    match created_by {
        CreatedBy::APrincipal => assert!(
            arn.starts_with("arn:aws:iam::") && arn.len() >= 20,
            "case {case:?}: CreatedByArn must be an IAM ARN of at least the modelled 20 \
             characters, got {arn:?} in {metadata}"
        ),
        CreatedBy::Nobody => assert_eq!(
            arn, "",
            "case {case:?}: CreatedByArn must be empty while no principal has written the \
             configuration, got {arn:?} in {metadata}"
        ),
    }
    let created = at(metadata, &["CreationDateTime"])
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: CreationDateTime must be a timestamp, got {metadata}"));
    assert!(
        (at_least..=at_most).contains(&created),
        "case {case:?}: CreationDateTime {created} must fall in {at_least}..={at_most} \
         ({metadata})"
    );
}

/// Who the metadata under assertion must attribute the configuration to.
#[derive(Debug, Clone, Copy)]
enum CreatedBy {
    /// A `PutBlockPublicAccessConfiguration` wrote it, so the metadata names its caller.
    APrincipal,
    /// Nothing has written it: the configuration is still the account default, which no
    /// principal created or last modified.
    Nobody,
}

/// The class this suite is about: a configuration a caller may put. The rows differ
/// along what the rule generalises over — the flag's value, whether the exception list
/// is absent, empty or populated, how many ranges it holds, whether a range gives a
/// `MaxRange` at all, where its bounds sit, and in what order the ranges arrive.
fn configurations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "blocking on, exception list absent",
            json!({ "BlockPublicSecurityGroupRules": true }),
        ),
        (
            "blocking on, exception list empty",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [],
            }),
        ),
        (
            "blocking off, exception list absent",
            json!({ "BlockPublicSecurityGroupRules": false }),
        ),
        (
            "blocking off, exception list populated",
            json!({
                "BlockPublicSecurityGroupRules": false,
                "PermittedPublicSecurityGroupRuleRanges": [
                    { "MinRange": 8443, "MaxRange": 8443 },
                ],
            }),
        ),
        (
            // "To specify a single port, use the same value for MinRange and MaxRange."
            // (PortRange doc.)
            "a single port given as both bounds",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [
                    { "MinRange": DEFAULT_PERMITTED_PORT, "MaxRange": DEFAULT_PERMITTED_PORT },
                ],
            }),
        ),
        (
            "a range with no MaxRange at all",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [{ "MinRange": 23 }],
            }),
        ),
        (
            "the published port bounds, as one range",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [
                    { "MinRange": PORT_MIN, "MaxRange": PORT_MAX },
                ],
            }),
        ),
        (
            "each published bound as its own single port",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [
                    { "MinRange": PORT_MIN, "MaxRange": PORT_MIN },
                    { "MinRange": PORT_MAX, "MaxRange": PORT_MAX },
                ],
            }),
        ),
        (
            "five disjoint ranges, in descending order",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [
                    { "MinRange": 9000, "MaxRange": 9999 },
                    { "MinRange": 8080, "MaxRange": 8088 },
                    { "MinRange": 443, "MaxRange": 443 },
                    { "MinRange": 80, "MaxRange": 80 },
                    { "MinRange": 22, "MaxRange": 22 },
                ],
            }),
        ),
        (
            "adjacent ranges that touch without overlapping",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [
                    { "MinRange": 20, "MaxRange": 21 },
                    { "MinRange": 22, "MaxRange": 23 },
                    { "MinRange": 24 },
                ],
            }),
        ),
        (
            "a configuration carrying the classification members too",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [{ "MinRange": 22, "MaxRange": 22 }],
                "Classification": "core-site",
                "Properties": { "fs.s3.enableServerSideEncryption": "true" },
            }),
        ),
    ]
}

/// THE RULE: `PutBlockPublicAccessConfiguration` replaces the account's single block
/// public access configuration whole, and `GetBlockPublicAccessConfiguration` reports
/// that stored configuration — every member the caller supplied, no member it did not,
/// and the exception list it left out as the empty list that means the same thing.
#[tokio::test]
async fn a_put_configuration_is_what_a_later_get_reports() {
    let emr = Emr::new();

    for (case, configuration) in configurations() {
        let before = epoch_secs(SystemTime::now()) - 1.0;
        put(&emr, case, &configuration).await;
        let after = epoch_secs(SystemTime::now()) + 1.0;

        let output = get(&emr, case).await;
        let (reported, metadata) = parts(&output);
        assert_eq!(
            reported,
            &canonical(&configuration),
            "case {case:?}: the reported configuration must be the one that was put, \
             got {reported}"
        );
        assert_metadata(case, metadata, CreatedBy::APrincipal, before, after);
    }
}

/// THE RULE: a put REPLACES the stored configuration rather than merging into it, so a
/// member the new configuration leaves out is absent afterwards — the exception list it
/// leaves out being the empty list, which is what "no exception" reads back as.
#[tokio::test]
async fn a_put_replaces_the_stored_configuration_rather_than_merging_into_it() {
    let emr = Emr::new();

    // Every row is put over the row before it; after each one the whole configuration
    // is what that row said, carrying nothing from its predecessor.
    for (case, configuration) in [
        (
            "three permitted ranges",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [
                    { "MinRange": 22, "MaxRange": 22 },
                    { "MinRange": 80, "MaxRange": 88 },
                    { "MinRange": 443, "MaxRange": 443 },
                ],
            }),
        ),
        (
            "one permitted range replaces three",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [{ "MinRange": 443, "MaxRange": 443 }],
            }),
        ),
        (
            "an empty list replaces one range",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [],
            }),
        ),
        (
            "no list at all replaces the empty list",
            json!({ "BlockPublicSecurityGroupRules": false }),
        ),
        (
            "a list again, with the flag back on",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [{ "MinRange": 9000 }],
            }),
        ),
    ] {
        put(&emr, case, &configuration).await;
        let output = get(&emr, case).await;
        let (reported, _) = parts(&output);
        assert_eq!(
            reported,
            &canonical(&configuration),
            "case {case:?}: a put replaces the configuration whole, got {reported}"
        );
    }
}

/// THE RULE: once a configuration has been put, the account HAS an exception list: a put
/// that omits `PermittedPublicSecurityGroupRuleRanges` clears the ranges and reads back
/// as the empty list — the same canonical representation an explicit `[]` reads back as —
/// never as an absent member a client cannot tell from "unknown".
#[tokio::test]
async fn an_omitted_exception_list_reads_back_as_the_empty_list() {
    // Each row is the exception list a FIRST put establishes; the second put of every row
    // omits the member, and the stored list must then be empty whatever it held before.
    for (case, established) in [
        (
            "the recorded pair of ranges",
            json!([{ "MinRange": 22, "MaxRange": 22 }, { "MinRange": 8000, "MaxRange": 8010 }]),
        ),
        ("one single-port range", json!([{ "MinRange": 443, "MaxRange": 443 }])),
        ("a range with no MaxRange", json!([{ "MinRange": 9000 }])),
        (
            "the whole published port space",
            json!([{ "MinRange": PORT_MIN, "MaxRange": PORT_MAX }]),
        ),
        ("an explicitly empty list", json!([])),
    ] {
        let emr = Emr::new();
        put(
            &emr,
            case,
            &json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": established,
            }),
        )
        .await;
        assert_eq!(
            at(parts(&get(&emr, case).await).0, &["PermittedPublicSecurityGroupRuleRanges"]),
            &established,
            "case {case:?}: the first put must establish the ranges"
        );

        // The same configuration again with the member left out: a full replace.
        put(&emr, case, &json!({ "BlockPublicSecurityGroupRules": true })).await;
        let reported = parts(&get(&emr, case).await).0.clone();
        assert_eq!(
            at(&reported, &["PermittedPublicSecurityGroupRuleRanges"]),
            &json!([]),
            "case {case:?}: omitting the list clears it to the empty list, got {reported}"
        );

        // And an explicit empty list afterwards reports the very same configuration, so
        // the two forms are ONE representation.
        put(
            &emr,
            case,
            &json!({
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [],
            }),
        )
        .await;
        assert_eq!(
            parts(&get(&emr, case).await).0,
            &reported,
            "case {case:?}: the omitted and the explicitly empty list must read back alike"
        );
    }
}

/// THE RULE: an account that has never put a configuration still has one — block public
/// access enabled, with Port 22 permitted — reading it changes nothing, and its metadata
/// attributes it to NO principal, because none created or last modified it.
#[tokio::test]
async fn an_account_that_never_put_a_configuration_reports_the_documented_default() {
    let emr = Emr::new();
    let case = "the account default";
    let now = epoch_secs(SystemTime::now()) + 1.0;

    let first = get(&emr, case).await;
    let (configuration, metadata) = parts(&first);
    assert_eq!(
        at(configuration, &["BlockPublicSecurityGroupRules"]),
        &json!(true),
        "case {case:?}: the default blocks public security group rules, got {configuration}"
    );
    assert_eq!(
        at(configuration, &["PermittedPublicSecurityGroupRuleRanges"]),
        &json!([{ "MinRange": DEFAULT_PERMITTED_PORT, "MaxRange": DEFAULT_PERMITTED_PORT }]),
        "case {case:?}: the default permits Port 22, got {configuration}"
    );
    // 0.0 as the floor: the default was established before this call, at an instant this
    // test cannot name, so only its upper bound is assertable.
    assert_metadata(case, metadata, CreatedBy::Nobody, 0.0, now);

    // A read is a read: the second call reports exactly what the first did, metadata
    // included.
    let second = get(&emr, case).await;
    assert_eq!(
        second, first,
        "case {case:?}: repeated reads must report the same configuration and metadata, \
         got {second} then {first}"
    );
}

/// THE RULE: the metadata reports the principal that put the configuration and the
/// instant of that put, and it is updated on every put.
#[tokio::test]
async fn the_metadata_follows_the_put_that_wrote_the_configuration() {
    let emr = Emr::new();

    let default_metadata = parts(&get(&emr, "default").await).1.clone();

    let mut previous = default_metadata.clone();
    for (case, flag) in [("first put", true), ("second put", false), ("third put", true)] {
        let before = epoch_secs(SystemTime::now()) - 1.0;
        put(&emr, case, &json!({ "BlockPublicSecurityGroupRules": flag }))
            .await;
        let after = epoch_secs(SystemTime::now()) + 1.0;

        let metadata = parts(&get(&emr, case).await).1.clone();
        assert_metadata(case, &metadata, CreatedBy::APrincipal, before, after);
        assert_ne!(
            at(&metadata, &["CreatedByArn"]),
            at(&default_metadata, &["CreatedByArn"]),
            "case {case:?}: a put records the calling principal, not the account default's, \
             got {metadata}"
        );
        assert!(
            at(&metadata, &["CreationDateTime"]).as_f64()
                >= at(&previous, &["CreationDateTime"]).as_f64(),
            "case {case:?}: each put moves the metadata instant forward, got {metadata} \
             after {previous}"
        );
        previous = metadata;
    }
}

/// THE RULE: `CreatedByArn` is derived from the request that put the configuration, so
/// two callers signing with different access key ids are reported as different
/// principals.
#[tokio::test]
async fn the_metadata_names_the_access_key_the_put_was_signed_with() {
    let emr = Emr::new();

    // Rows differ only in who signs: the same configuration is put each time, so any
    // difference in the reported principal comes from the request's credential.
    for (case, akid) in [
        ("an unsigned caller", None),
        ("one access key", Some("AKIAEXAMPLEONE")),
        ("another access key", Some("AKIAEXAMPLETWO")),
        ("the first access key again", Some("AKIAEXAMPLEONE")),
    ] {
        let (status, payload) = emr
            .call_as(
                akid,
                "PutBlockPublicAccessConfiguration",
                json!({
                    "BlockPublicAccessConfiguration": { "BlockPublicSecurityGroupRules": true },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the put must answer 200, got {status} with {payload}"
        );

        let metadata = parts(&get(&emr, case).await).1.clone();
        // The principal an unsigned request is attributed to is the `default` user —
        // what `engine::extract_akid` answers when a request carries no credential.
        let expected = format!("user/{}", akid.unwrap_or("default"));
        let arn = at(&metadata, &["CreatedByArn"]).as_str().unwrap_or_default();
        assert!(
            arn.ends_with(&expected),
            "case {case:?}: CreatedByArn must name {expected:?}, got {arn:?} in {metadata}"
        );
    }
}

/// THE RULE: an exception list is a set of upward port ranges, so a range that runs
/// downwards or two ranges that permit the same port are refused with
/// `InvalidRequestException` (HTTP 400), and the stored configuration is left alone.
#[tokio::test]
async fn an_exception_list_that_is_not_a_set_of_upward_ranges_is_refused() {
    let emr = Emr::new();

    // The configuration every refusal below must leave in place.
    let stored = json!({
        "BlockPublicSecurityGroupRules": true,
        "PermittedPublicSecurityGroupRuleRanges": [{ "MinRange": 22, "MaxRange": 22 }],
    });
    put(&emr, "the configuration a refusal must not disturb", &stored)
        .await;

    for (case, ranges) in [
        ("a range that runs downwards", json!([{ "MinRange": 100, "MaxRange": 50 }])),
        (
            "a range that runs from the top bound to the bottom one",
            json!([{ "MinRange": PORT_MAX, "MaxRange": PORT_MIN }]),
        ),
        (
            "one range inside another",
            json!([{ "MinRange": 20, "MaxRange": 30 }, { "MinRange": 22, "MaxRange": 23 }]),
        ),
        (
            "two ranges that straddle each other",
            json!([{ "MinRange": 20, "MaxRange": 25 }, { "MinRange": 25, "MaxRange": 30 }]),
        ),
        (
            "the same single port twice",
            json!([{ "MinRange": 80, "MaxRange": 80 }, { "MinRange": 80, "MaxRange": 80 }]),
        ),
        (
            "a MaxRange-less range inside another",
            json!([{ "MinRange": 25, "MaxRange": 30 }, { "MinRange": 27 }]),
        ),
        (
            "an overlap hidden behind four disjoint ranges",
            json!([
                { "MinRange": 1, "MaxRange": 9 },
                { "MinRange": 10, "MaxRange": 19 },
                { "MinRange": 20, "MaxRange": 29 },
                { "MinRange": 30, "MaxRange": 39 },
                { "MinRange": 35, "MaxRange": 35 },
            ]),
        ),
    ] {
        let (status, payload) = emr
            .call(
                "PutBlockPublicAccessConfiguration",
                json!({
                    "BlockPublicAccessConfiguration": {
                        "BlockPublicSecurityGroupRules": true,
                        "PermittedPublicSecurityGroupRuleRanges": ranges,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        let code = at(&payload, &["__type"]).as_str().unwrap_or_default();
        assert!(
            code.contains("InvalidRequestException"),
            "case {case:?}: expected error code InvalidRequestException, got {code:?} in \
             {payload}"
        );

        let reported = parts(&get(&emr, case).await).0.clone();
        assert_eq!(
            reported, stored,
            "case {case:?}: a refused put stores nothing, got {reported}"
        );
    }
}

/// THE RULE: a port bound outside `Port`'s published range is refused, and one on either
/// bound is accepted.
#[tokio::test]
async fn a_port_bound_outside_the_modelled_range_is_refused() {
    let emr = Emr::new();

    for (case, ranges, expected) in [
        (
            "both bounds at the bottom of the published range",
            json!([{ "MinRange": PORT_MIN, "MaxRange": PORT_MIN }]),
            200u16,
        ),
        (
            "both bounds at the top of the published range",
            json!([{ "MinRange": PORT_MAX, "MaxRange": PORT_MAX }]),
            200,
        ),
        (
            "a MinRange one below the published bottom",
            json!([{ "MinRange": PORT_MIN - 1, "MaxRange": 22 }]),
            400,
        ),
        (
            "a MaxRange one above the published top",
            json!([{ "MinRange": 22, "MaxRange": PORT_MAX + 1 }]),
            400,
        ),
    ] {
        let (status, payload) = emr
            .call(
                "PutBlockPublicAccessConfiguration",
                json!({
                    "BlockPublicAccessConfiguration": {
                        "BlockPublicSecurityGroupRules": true,
                        "PermittedPublicSecurityGroupRuleRanges": ranges,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {payload}"
        );
    }
}

/// THE RULE: the exception list is bounded by the port space it draws from — the whole
/// space as single ports is accepted and round-trips, and a list longer than that
/// cannot be disjoint and is refused.
#[tokio::test]
async fn an_exception_list_is_bounded_by_the_port_space_it_draws_from() {
    let emr = Emr::new();

    // Every port the model admits, each as its own range: the largest disjoint list
    // there is, and the size at which an all-pairs overlap check would be ruinous.
    let whole_space: Vec<Value> = (PORT_MIN..=PORT_MAX)
        .map(|port| json!({ "MinRange": port, "MaxRange": port }))
        .collect();
    let ports = whole_space.len();

    let case = "every port as its own range";
    let configuration = json!({
        "BlockPublicSecurityGroupRules": true,
        "PermittedPublicSecurityGroupRuleRanges": whole_space.clone(),
    });
    put(&emr, case, &configuration).await;
    let reported = parts(&get(&emr, case).await).0.clone();
    assert_eq!(
        at(&reported, &["PermittedPublicSecurityGroupRuleRanges"])
            .as_array()
            .map(Vec::len),
        Some(ports),
        "case {case:?}: all {ports} ranges must round-trip, got {} of them",
        at(&reported, &["PermittedPublicSecurityGroupRuleRanges"])
            .as_array()
            .map_or(0, Vec::len)
    );
    assert_eq!(
        reported, configuration,
        "case {case:?}: the reported configuration must be the one that was put"
    );

    // One range more than there are ports to permit: whatever it carries, two of its
    // ranges share a port, so the length alone refuses it.
    let case = "one range more than there are ports";
    let mut too_many = whole_space;
    too_many.push(json!({ "MinRange": DEFAULT_PERMITTED_PORT, "MaxRange": DEFAULT_PERMITTED_PORT }));
    let (status, payload) = emr
        .call(
            "PutBlockPublicAccessConfiguration",
            json!({
                "BlockPublicAccessConfiguration": {
                    "BlockPublicSecurityGroupRules": true,
                    "PermittedPublicSecurityGroupRuleRanges": too_many,
                },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    let code = at(&payload, &["__type"]).as_str().unwrap_or_default();
    assert!(
        code.contains("InvalidRequestException"),
        "case {case:?}: expected error code InvalidRequestException, got {code:?} in {payload}"
    );

    // And the refused put stored nothing: the configuration is still the accepted one.
    let reported = parts(&get(&emr, case).await).0.clone();
    assert_eq!(
        reported, configuration,
        "case {case:?}: a refused put stores nothing"
    );
}

/// THE RULE: a configuration carrying more entries than one may hold is refused, so no
/// single put can make every later read clone an unbounded body.
#[tokio::test]
async fn a_configuration_carrying_more_entries_than_the_cap_is_refused() {
    let emr = Emr::new();

    // The cap is the size of the port space (see `state::MAX_CONFIGURATION_ENTRIES`);
    // these rows sit either side of it, counting the entries of whichever member
    // carries them.
    let properties = |count: i64| -> Value {
        Value::Object(
            (0..count)
                .map(|at| (format!("emr.property.{at}"), json!("value")))
                .collect(),
        )
    };
    let entries_cap = PORT_MAX - PORT_MIN + 1;

    for (case, configuration, expected) in [
        (
            "a handful of properties, well inside the cap",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "Properties": properties(16),
            }),
            200u16,
        ),
        (
            "properties one over the cap",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "Properties": properties(entries_cap + 1),
            }),
            400,
        ),
        (
            "nested configurations one over the cap",
            json!({
                "BlockPublicSecurityGroupRules": true,
                "Configurations": (0..entries_cap + 1)
                    .map(|at| json!({ "Classification": format!("core-site-{at}") }))
                    .collect::<Vec<Value>>(),
            }),
            400,
        ),
    ] {
        let (status, payload) = emr
            .call(
                "PutBlockPublicAccessConfiguration",
                json!({ "BlockPublicAccessConfiguration": configuration }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {payload}"
        );
        if expected == 400 {
            let code = at(&payload, &["__type"]).as_str().unwrap_or_default();
            assert!(
                code.contains("InvalidRequestException"),
                "case {case:?}: expected error code InvalidRequestException, got {code:?} in \
                 {payload}"
            );
        }
    }
}
