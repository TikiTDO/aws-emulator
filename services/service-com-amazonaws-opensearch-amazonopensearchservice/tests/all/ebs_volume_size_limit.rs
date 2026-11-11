// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal an EBS volume sized outside the range the service
//! publishes for an instance type is answered with, on both paths that size a domain's
//! storage: `CreateDomain` and `UpdateDomainConfig`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the published bounds are read through `DescribeInstanceTypeLimits` itself
//! ([`super::instance_type_limits::published_volume_size_range`]); this suite adds no
//! second copy of either.
//!
//! The rule under test: a volume size outside the `MinimumVolumeSize` ..
//! `MaximumVolumeSize` range the service publishes for the instance type and volume
//! sub-type asks for storage the service does not offer for that instance type, so it is
//! refused with the modelled `LimitExceededException` (`errorKind` client, `httpError`
//! 409, from `smithy_query(query="shape",shape="LimitExceededException")`) rather than
//! with a request-shape refusal — and the sentence names the two published bounds, the
//! instance type the volume would attach to and the engine release the domain runs.
//!
//! So the assertions span SETS of inputs: a table of instance sizes, volume sub-types
//! and engine releases of both engine lines, each probed below the published floor and
//! above the published maximum, with each row's bounds read from the service's own
//! catalog; the bounds themselves, which are accepted and read back; the update path,
//! where the merged record is what the bounds are read for; and, over a second table of
//! pairings, that a refused create materialises NO domain — neither reader answers from
//! a record and the enumeration does not hold the name, while the same name sized at the
//! published floor is created.

use serde_json::{json, Value};

use super::domain_lifecycle::{
    error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::instance_type_limits::published_volume_size_range;

/// The pairings the table spans: one row per instance SIZE the published maximum
/// differs for, per volume SUB-TYPE (including the magnetic one, whose maximum is a flat
/// cap rather than the instance's scale), across releases of both engine lines — so no
/// row rests on one instance type, one sub-type or one release.
const ROWS: [(&str, &str, &str); 6] = [
    ("OpenSearch_2.11", "t3.small.search", "gp3"),
    ("OpenSearch_1.0", "m5.large.search", "gp2"),
    ("OpenSearch_2.19", "m4.2xlarge.search", "io1"),
    ("Elasticsearch_7.10", "m5.xlarge.search", "gp3"),
    ("Elasticsearch_7.10", "m4.large.search", "standard"),
    ("OpenSearch_2.11", "t3.nano.search", "gp3"),
];

/// This suite's OWN statement of the refusal text: the fixed sentence, whose variable
/// parts are the two bounds the service publishes for the pairing, the instance type the
/// request named and the engine release the domain runs. Restated here rather than
/// imported from the service, so changing it in the service fails this suite instead of
/// being asserted against itself.
fn expected_message(
    minimum: i64,
    maximum: i64,
    instance_type: &str,
    engine_version: &str,
) -> String {
    format!(
        "Volume size must be between {minimum} and {maximum} for {instance_type} instance \
         type and version {engine_version}"
    )
}

/// The error message of a refused answer, with the case named on failure.
fn message(answer: &Answer, case: &str) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case}: expected an error message, got {:?}", answer.body))
        .to_owned()
}

/// Assert one answer IS the service-limit refusal the rule prescribes, worded for the
/// published bounds and the pairing they are published for.
fn assert_refused(answer: &Answer, expected: &str, case: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case}: expected HTTP 409, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        "LimitExceededException",
        "case {case}: expected the modelled service-limit refusal, got {:?}",
        answer.body
    );
    let actual = message(answer, case);
    assert_eq!(
        actual, expected,
        "case {case}: expected the published bounds, the instance type and the release to \
         be named, got {actual:?}"
    );
}

/// A `CreateDomain` body for one pairing and one volume size.
fn create_body(
    name: &str,
    engine_version: &str,
    instance_type: &str,
    sub_type: &str,
    size: i64,
) -> Value {
    json!({
        "DomainName": name,
        "EngineVersion": engine_version,
        "ClusterConfig": { "InstanceType": instance_type, "InstanceCount": 1 },
        "EBSOptions": { "EBSEnabled": true, "VolumeType": sub_type, "VolumeSize": size },
    })
}

#[tokio::test]
async fn a_volume_outside_the_published_range_is_a_service_limit_refusal_on_create() {
    let emulator = Emulator::new();

    for (row, (engine_version, instance_type, sub_type)) in ROWS.into_iter().enumerate() {
        let (minimum, maximum) =
            published_volume_size_range(&emulator, engine_version, instance_type, sub_type).await;
        let expected = expected_message(minimum, maximum, instance_type, engine_version);

        // Probed at both ends of the published range and one past each end, plus the
        // smallest size a request can carry at all: the refusal is the same whichever
        // bound the size is outside, and the bounds themselves are inside it.
        let probes: [(i64, bool); 5] = [
            (minimum - 1, false),
            (1, false),
            (minimum, true),
            (maximum, true),
            (maximum + 1, false),
        ];
        for (index, (size, accepted)) in probes.into_iter().enumerate() {
            // A distinct domain per probe, so no probe is answered by an earlier one's
            // record.
            let name = format!("ebs-limit-{row}-{index}");
            let case = format!("{engine_version} {instance_type} {sub_type} {size} GiB");
            let answer = emulator
                .create(create_body(
                    &name,
                    engine_version,
                    instance_type,
                    sub_type,
                    size,
                ))
                .await;
            if accepted {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected a published bound to be accepted, got {} {:?}",
                    answer.status, answer.body
                );
                let stored = emulator.describe(&name).await;
                assert_eq!(
                    stored.body["DomainStatus"]["EBSOptions"]["VolumeSize"],
                    json!(size),
                    "case {case}: expected the accepted volume to be read back, got {:?}",
                    stored.body["DomainStatus"]["EBSOptions"]
                );
            } else {
                assert_refused(&answer, &expected, &case);
                // A refused create stores nothing.
                let stored = emulator.describe(&name).await;
                assert_eq!(
                    stored.status, RECORDED_NOT_FOUND_STATUS,
                    "case {case}: expected no domain to have been created, got {} {:?}",
                    stored.status, stored.body
                );
            }
        }
    }
}

#[tokio::test]
async fn the_refusal_names_the_release_the_request_asked_for() {
    let emulator = Emulator::new();
    let instance_type = "t3.small.search";
    let sub_type = "gp3";

    // One release per engine line, with the same undersized volume on the same instance
    // type: the release named in the sentence is the one the REQUEST carried.
    for (index, engine_version) in ["OpenSearch_2.11", "Elasticsearch_7.10", "OpenSearch_1.3"]
        .into_iter()
        .enumerate()
    {
        let (minimum, maximum) =
            published_volume_size_range(&emulator, engine_version, instance_type, sub_type).await;
        let case = format!("{engine_version} undersized");
        let answer = emulator
            .create(create_body(
                &format!("ebs-release-{index}"),
                engine_version,
                instance_type,
                sub_type,
                minimum - 1,
            ))
            .await;
        assert_refused(
            &answer,
            &expected_message(minimum, maximum, instance_type, engine_version),
            &case,
        );
    }
}

#[tokio::test]
async fn a_volume_outside_the_published_range_is_a_service_limit_refusal_on_update() {
    let emulator = Emulator::new();
    let engine_version = "OpenSearch_2.11";
    // A family that publishes every volume sub-type, so each row below is judged against
    // bounds the service publishes for the pairing it names.
    let instance_type = "m4.large.search";
    let name = "ebs-limit-resize";
    let (minimum, maximum) =
        published_volume_size_range(&emulator, engine_version, instance_type, "gp3").await;

    let created = emulator
        .create(create_body(name, engine_version, instance_type, "gp3", 20))
        .await;
    assert_eq!(
        created.status, 200,
        "case create {name}: expected HTTP 200, got {} {:?}",
        created.status, created.body
    );
    // The service applies one change of a domain at a time, so the change the create
    // started has to land before an update can start another.
    tokio::time::sleep(SETTLE_WAIT).await;

    // The bounds an update is judged against are the ones published for the MERGED
    // record: a resize is judged against the instance type the domain keeps, and a
    // change of instance type against the volume it keeps — so each row states the
    // pairing whose bounds the refusal names.
    let smaller = "t3.small.search";
    let (small_minimum, small_maximum) =
        published_volume_size_range(&emulator, engine_version, smaller, "gp3").await;
    let (magnetic_minimum, magnetic_maximum) =
        published_volume_size_range(&emulator, engine_version, instance_type, "standard").await;
    let rows: [(&str, Value, String); 4] = [
        (
            "resize below the published floor",
            json!({
                "EBSOptions": {
                    "EBSEnabled": true,
                    "VolumeType": "gp3",
                    "VolumeSize": minimum - 1,
                }
            }),
            expected_message(minimum, maximum, instance_type, engine_version),
        ),
        (
            "resize over the published maximum",
            json!({
                "EBSOptions": {
                    "EBSEnabled": true,
                    "VolumeType": "gp3",
                    "VolumeSize": maximum + 1,
                }
            }),
            expected_message(minimum, maximum, instance_type, engine_version),
        ),
        (
            "a magnetic volume over the flat cap",
            json!({
                "EBSOptions": {
                    "EBSEnabled": true,
                    "VolumeType": "standard",
                    "VolumeSize": magnetic_maximum + 1,
                }
            }),
            expected_message(
                magnetic_minimum,
                magnetic_maximum,
                instance_type,
                engine_version,
            ),
        ),
        (
            "a volume over the maximum of the instance type the request names",
            json!({
                "EBSOptions": {
                    "EBSEnabled": true,
                    "VolumeType": "gp3",
                    "VolumeSize": small_maximum + 1,
                },
                "ClusterConfig": { "InstanceType": smaller, "InstanceCount": 1 },
            }),
            expected_message(small_minimum, small_maximum, smaller, engine_version),
        ),
    ];

    for (case, body, expected) in rows {
        let answer = emulator.update_config(name, body.clone()).await;
        assert_refused(&answer, &expected, case);
        // A refused update stores nothing: the domain keeps the volume it had.
        let stored = emulator.describe(name).await;
        assert_eq!(
            stored.body["DomainStatus"]["EBSOptions"]["VolumeSize"],
            json!(20),
            "case {case}: expected the stored volume to be untouched, got {:?}",
            stored.body["DomainStatus"]["EBSOptions"]
        );
        // A dry run reports what applying the change would do, so a change that cannot
        // be applied is refused there in the same class.
        let mut dry = body;
        dry["DryRun"] = json!(true);
        let answer = emulator.update_config(name, dry).await;
        assert_refused(&answer, &expected, &format!("{case} dry run"));
    }

    // A resize INSIDE the published range is applied and read back, so the refusal is
    // exactly the range's complement rather than a refusal of every resize.
    let answer = emulator
        .update_config(
            name,
            json!({
                "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": maximum }
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case resize to the published maximum: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    let stored = emulator.describe(name).await;
    assert_eq!(
        stored.body["DomainStatus"]["EBSOptions"]["VolumeSize"],
        json!(maximum),
        "case resize to the published maximum: expected the new volume size, got {:?}",
        stored.body["DomainStatus"]["EBSOptions"]
    );
}

#[tokio::test]
async fn a_request_naming_no_instance_type_is_held_to_the_published_floor() {
    let emulator = Emulator::new();
    let engine_version = "OpenSearch_2.11";
    // The floor is published for every instance type alike, so it is the one bound a
    // request that pairs its volume with no instance type is held to.
    let (minimum, _) =
        published_volume_size_range(&emulator, engine_version, "m5.large.search", "gp3").await;

    let answer = emulator
        .create(json!({
            "DomainName": "ebs-no-instance-type",
            "EngineVersion": engine_version,
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": minimum - 1 },
        }))
        .await;
    assert_eq!(
        answer.status, 409,
        "case no instance type: expected HTTP 409, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "LimitExceededException",
        "case no instance type: expected the modelled service-limit refusal, got {:?}",
        answer.body
    );
    let actual = message(&answer, "no instance type");
    assert_eq!(
        actual,
        format!("Volume size must be at least {minimum} for version {engine_version}"),
        "case no instance type: expected the floor and the release to be named and no \
         instance type, got {actual:?}"
    );

    // EBS not enabled attaches no volume, so there is no volume to size and nothing to
    // hold to the floor.
    let answer = emulator
        .create(json!({
            "DomainName": "ebs-not-enabled",
            "EngineVersion": engine_version,
            "EBSOptions": { "EBSEnabled": false, "VolumeSize": 1 },
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case EBS not enabled: expected no volume to be sized, got {} {:?}",
        answer.status, answer.body
    );
}

/// The pairings the "nothing was created" invariant is held over: a row per instance
/// FAMILY, volume sub-type and engine line, so the invariant rests on no single
/// instance type, sub-type or release.
const ABSENCE_ROWS: [(&str, &str, &str); 3] = [
    ("OpenSearch_2.19", "r6g.large.search", "gp2"),
    ("Elasticsearch_7.10", "m4.4xlarge.search", "io1"),
    ("OpenSearch_2.11", "t3.small.search", "gp3"),
];

/// The domain names `ListDomainNames` enumerates for the account.
async fn listed_names(emulator: &Emulator, case: &str) -> Vec<String> {
    let answer = emulator.call("GET", &format!("{PREFIX}/domain"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected ListDomainNames to answer 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["DomainNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected DomainNames, got {:?}", answer.body))
        .iter()
        .filter_map(|entry| entry["DomainName"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_create_refused_by_the_published_bounds_materialises_no_domain() {
    let emulator = Emulator::new();

    for (row, (engine_version, instance_type, sub_type)) in ABSENCE_ROWS.into_iter().enumerate() {
        let (minimum, maximum) =
            published_volume_size_range(&emulator, engine_version, instance_type, sub_type).await;
        let expected = expected_message(minimum, maximum, instance_type, engine_version);

        // One probe past each published bound: whichever bound the size is outside, the
        // request is refused and the account is left exactly as it was.
        for (index, size) in [minimum - 1, maximum + 1].into_iter().enumerate() {
            let name = format!("ebs-absent-{row}-{index}");
            let case = format!("{engine_version} {instance_type} {sub_type} {size} GiB");
            let answer = emulator
                .create(create_body(
                    &name,
                    engine_version,
                    instance_type,
                    sub_type,
                    size,
                ))
                .await;
            assert_refused(&answer, &expected, &case);

            // Nothing was materialised, which is observable through every reader of a
            // domain: neither read answers from a record, and the enumeration does not
            // hold the name.
            for (reader, answer) in [
                ("DescribeDomain", emulator.describe(&name).await),
                ("DescribeDomainConfig", emulator.describe_config(&name).await),
            ] {
                assert_eq!(
                    answer.status, RECORDED_NOT_FOUND_STATUS,
                    "case {case} {reader}: expected no domain to have been created, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    error_code(&answer),
                    "ResourceNotFoundException",
                    "case {case} {reader}: expected the not-found refusal, got {:?}",
                    answer.body
                );
                let actual = message(&answer, &case);
                assert_eq!(
                    actual,
                    format!("Domain not found: {name}"),
                    "case {case} {reader}: expected the missing name to be reported, got \
                     {actual:?}"
                );
            }
            let listed = listed_names(&emulator, &case).await;
            assert!(
                !listed.contains(&name),
                "case {case}: expected {name} not to be enumerated, got {listed:?}"
            );
        }

        // The refusal took nothing, not even the name: the SAME name, sized at the
        // published floor, is created and then enumerated — so the invariant above is
        // the refusal's own, not a service that creates no domain at all.
        let name = format!("ebs-absent-{row}-0");
        let case = format!("{engine_version} {instance_type} {sub_type} {minimum} GiB accepted");
        let answer = emulator
            .create(create_body(
                &name,
                engine_version,
                instance_type,
                sub_type,
                minimum,
            ))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the published floor to be accepted, got {} {:?}",
            answer.status, answer.body
        );
        let listed = listed_names(&emulator, &case).await;
        assert!(
            listed.contains(&name),
            "case {case}: expected {name} to be enumerated, got {listed:?}"
        );
    }
}
