// SPDX-License-Identifier: CC0-1.0

//! `CreatePlatformVersion` BUILDS the version from its definition archive, so a
//! custom platform version reaches a terminal `PlatformStatus` that says whether
//! the build succeeded instead of always reporting success.
//!
//! RULE under test: "create a new version of your custom platform" builds that
//! version from `PlatformDefinitionBundle`, "the location of the platform
//! definition archive in Amazon S3", and the archive is built from the PLATFORM
//! DEFINITION FILE at its root (`platform.yaml`) — so the version reports
//! `Creating` until the build is due, then `Ready` when the archive carries a
//! definition file that parses and `Failed` when it carries none, carries one
//! that does not parse, or is no archive at all. A version that failed to build
//! is no platform to launch on, so an environment named onto it is refused.
//!
//! The verdict is reached by READING the archive, so the rows span what the rule
//! generalises over: whether the archive carries the definition file at all,
//! where in the archive it sits, whether it is a YAML document, whether it is
//! text, how many other entries the archive holds, and whether the bundle is an
//! archive at all. Every suite here mounts the S3 service beside Elastic
//! Beanstalk on ONE engine and uploads the bundle through the S3 front door, so
//! the archive the build reads is one object storage really holds.

use crate::application_version_processing::Bundle;
use crate::harness::{Answer, Emulator, PAST_DUE};
use crate::lifecycle::TEST_REGION;

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The bucket the suites upload a platform definition bundle into, and the key
/// under it — nested, the shape a client's build pipeline uploads under.
const BUNDLE_BUCKET: &str = "eb-platform-bundles";
/// See [`BUNDLE_BUCKET`].
const BUNDLE_KEY: &str = "platforms/2026/definition.zip";

/// The platform name and version every row creates.
const PLATFORM_NAME: &str = "build-platform";
/// See [`PLATFORM_NAME`].
const PLATFORM_VERSION: &str = "1.0.0";

/// A platform definition file in the form the custom-platform documentation
/// gives it: a mapping of the platform's own keys.
const VALID_MANIFEST: &[u8] = b"version: \"1.0\"\nprovisioner:\n  type: packer\n";

/// Text that is not a YAML document: a flow mapping that is never closed.
const BROKEN_YAML: &[u8] = b"provisioner: {\n";

/// Bytes that are not text at all, so no definition can be parsed out of them.
const NOT_TEXT: &[u8] = b"\x00\xff\xfe\x01platform";

/// One definition bundle a create names, and the terminal `PlatformStatus` the
/// version must settle in for it.
struct Row {
    case: &'static str,
    bundle: Bundle,
    /// The modelled status, taken from the service's own constants so the
    /// expectation cannot drift from what the service reports.
    terminal: &'static str,
}

/// One row per thing the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        case: "a bundle whose platform.yaml is a YAML document",
        bundle: Bundle::Zip(&[("platform.yaml", VALID_MANIFEST)]),
        terminal: eb::state::PLATFORM_STATUS_READY,
    },
    Row {
        case: "a bundle carrying the definition file beside the builder's own files",
        bundle: Bundle::Zip(&[
            ("platform.yaml", VALID_MANIFEST),
            ("builder/install.sh", b"#!/bin/sh\necho ok\n"),
            ("builder/platform.json", b"{\"builders\": []}"),
        ]),
        terminal: eb::state::PLATFORM_STATUS_READY,
    },
    Row {
        case: "a bundle carrying no platform definition file",
        bundle: Bundle::Zip(&[
            ("Procfile", b"web: python3 server.py"),
            ("builder/install.sh", b"#!/bin/sh\necho ok\n"),
        ]),
        terminal: eb::state::PLATFORM_STATUS_FAILED,
    },
    Row {
        case: "an archive with no entries at all",
        bundle: Bundle::Zip(&[]),
        terminal: eb::state::PLATFORM_STATUS_FAILED,
    },
    Row {
        // The definition file is read at the archive's ROOT, so one nested
        // under a folder is not the platform's definition.
        case: "a bundle whose platform.yaml sits under a folder",
        bundle: Bundle::Zip(&[("custom/platform.yaml", VALID_MANIFEST)]),
        terminal: eb::state::PLATFORM_STATUS_FAILED,
    },
    Row {
        case: "a bundle whose platform.yaml is not a YAML document",
        bundle: Bundle::Zip(&[("platform.yaml", BROKEN_YAML)]),
        terminal: eb::state::PLATFORM_STATUS_FAILED,
    },
    Row {
        case: "a bundle whose platform.yaml is not text",
        bundle: Bundle::Zip(&[("platform.yaml", NOT_TEXT)]),
        terminal: eb::state::PLATFORM_STATUS_FAILED,
    },
    Row {
        case: "a bundle that is not an archive",
        bundle: Bundle::Raw(b"this is not an archive, it is a sentence"),
        terminal: eb::state::PLATFORM_STATUS_FAILED,
    },
];

/// `DescribePlatformVersion` for `platform_arn`.
async fn described(emulator: &Emulator, platform_arn: &str) -> Answer {
    emulator
        .call("DescribePlatformVersion", &[("PlatformArn", platform_arn)])
        .await
}

/// The `PlatformStatus` that operation reports, with the answer it was read
/// from for a failing assertion.
async fn described_status(emulator: &Emulator, platform_arn: &str) -> (String, Answer) {
    let answer = described(emulator, platform_arn).await;
    let status = answer
        .value("PlatformDescription/PlatformStatus")
        .unwrap_or_default()
        .to_string();
    (status, answer)
}

/// Create the custom platform version from the bundle object storage holds, and
/// answer the `PlatformArn` the create reported.
async fn create(emulator: &Emulator, case: &str) -> String {
    let created = emulator
        .call(
            "CreatePlatformVersion",
            &[
                ("PlatformName", PLATFORM_NAME),
                ("PlatformVersion", PLATFORM_VERSION),
                ("PlatformDefinitionBundle.S3Bucket", BUNDLE_BUCKET),
                ("PlatformDefinitionBundle.S3Key", BUNDLE_KEY),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePlatformVersion, got {} and body {}",
        created.status, created.body
    );
    // The create answers with the transient status: the build it started is not
    // done when the call returns.
    assert_eq!(
        created.value("PlatformSummary/PlatformStatus"),
        Some(eb::state::PLATFORM_STATUS_CREATING),
        "case {case:?}: expected the create to report {}, got body {}",
        eb::state::PLATFORM_STATUS_CREATING,
        created.body
    );
    created
        .expect("PlatformSummary/PlatformArn", case)
        .to_string()
}

#[tokio::test]
async fn a_platform_version_settles_in_the_status_the_build_of_its_bundle_reached() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::with_object_storage();
        emulator
            .put_object(BUNDLE_BUCKET, BUNDLE_KEY, &row.bundle.bytes())
            .await;

        let arn = create(&emulator, case).await;
        assert_eq!(
            arn,
            eb::state::platform_version_arn(TEST_REGION, PLATFORM_NAME, PLATFORM_VERSION),
            "case {case:?}: unexpected PlatformArn"
        );
        // A caller polling straight after the create still sees the build.
        let (immediately, answer) = described_status(&emulator, &arn).await;
        assert_eq!(
            immediately,
            eb::state::PLATFORM_STATUS_CREATING,
            "case {case:?}: expected {} immediately after the create, got {immediately:?} and \
             body {}",
            eb::state::PLATFORM_STATUS_CREATING,
            answer.body
        );

        std::thread::sleep(PAST_DUE);
        // The round trip, read twice: a read must not change what the next one
        // reports, and a terminal status is terminal.
        for read in 1..=2 {
            let (settled, answer) = described_status(&emulator, &arn).await;
            assert_eq!(
                settled, row.terminal,
                "case {case:?}: expected the version to settle in {} on read {read}, got \
                 {settled:?} and body {}",
                row.terminal, answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_platform_version_that_failed_to_build_is_not_a_platform_to_launch_on() {
    let case = "a bundle carrying no platform definition file";
    let emulator = Emulator::with_object_storage();
    emulator
        .put_object(
            BUNDLE_BUCKET,
            BUNDLE_KEY,
            &Bundle::Zip(&[("Procfile", b"web: true")]).bytes(),
        )
        .await;
    let arn = create(&emulator, case).await;
    std::thread::sleep(PAST_DUE);
    let (settled, answer) = described_status(&emulator, &arn).await;
    assert_eq!(
        settled,
        eb::state::PLATFORM_STATUS_FAILED,
        "case {case:?}: the build must have failed, got {settled:?} and body {}",
        answer.body
    );

    let created = emulator
        .call("CreateApplication", &[("ApplicationName", "failed-platform-app")])
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
    let refused = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "failed-platform-app"),
                ("EnvironmentName", "on-failed-platform"),
                ("PlatformArn", &arn),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400 from CreateEnvironment, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case {case:?}: unexpected error code, got body {}",
        refused.body
    );
    assert!(
        refused
            .error_message()
            .contains(eb::state::PLATFORM_STATUS_FAILED),
        "case {case:?}: the refusal must say what state the platform is in, got body {}",
        refused.body
    );
}
