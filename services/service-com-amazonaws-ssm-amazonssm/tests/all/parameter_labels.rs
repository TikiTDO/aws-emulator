// SPDX-License-Identifier: CC0-1.0

//! Parameter labels and the batch delete, end to end over the front door:
//! `LabelParameterVersion` attaches, `UnlabelParameterVersion` releases and
//! `DeleteParameters` removes, while `GetParameter` and `GetParameterHistory` observe
//! what was persisted.
//!
//! The rules under test:
//!
//! * **a label is attached to exactly one version of a parameter — the one the request
//!   names, or the newest when it names none — so a `name:label` read resolves to that
//!   version's value, attaching it to another version MOVES it, and a label that fails
//!   the documented label requirements is reported in `InvalidLabels` and attached to
//!   nothing;**
//! * **`UnlabelParameterVersion` reports the labels it actually took off the stored set
//!   in `RemovedLabels` and the ones that version did not hold in `InvalidLabels`, so a
//!   released label resolves to nothing again;**
//! * **`DeleteParameters` partitions the names it was given by what the store held: a
//!   name whose parameter was removed is reported deleted, every other name invalid.**
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body — through this crate's
//! one harness (`all/parameter_store.rs`). Nothing reaches into the state struct, so a
//! response that is not computed from persisted state cannot pass: every label read
//! follows the call that attached it.
//!
//! The tables' rows differ along what the rules generalise over: the parameter's name
//! form, the label's spelling, which version is named (and whether one is named at all),
//! how many labels a version already holds, how many names a batch delete carries and
//! what each of them names.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, ACCOUNT, REGION};

/// How many labels one version of a parameter holds: "A version of a parameter can have
/// a maximum of 10 labels"
/// (`smithy_query(query="doc", shape="LabelParameterVersion")`), which
/// `ParameterVersionLabelLimitExceeded` ("A parameter version can have a maximum of ten
/// labels") reports.
const MAX_LABELS_PER_VERSION: usize = 10;

/// The longest label a request may carry: "A label can have a maximum of 100 characters"
/// (the same doc), which is the modelled `ParameterLabel` length 1..100
/// (`smithy_query(query="shape", shape="ParameterLabel")`).
const MAX_LABEL_LENGTH: usize = 100;

/// One row of the attach/read/release suite.
struct LabelRow {
    case: &'static str,
    /// The name `PutParameter` writes the parameter under.
    written: &'static str,
    /// The name the label calls and the label read address it by — the same parameter,
    /// since "the leading slash is not part of the name" for a hierarchy path.
    addressed: &'static str,
    /// The label attached, spelled along the documented label requirements: "labels can
    /// contain letters (case sensitive), numbers, periods (.), hyphens (-), or
    /// underscores (_)".
    label: &'static str,
    /// The version the request names, or `None` for "if no version is specified, the
    /// system attaches the label to the latest version".
    parameter_version: Option<i64>,
    /// The version the label must end up on.
    expected_version: i64,
}

/// Three values, so every row has a version before, at and after the one it labels.
const VALUES: &[&str] = &["v1", "v2", "v3"];

const LABEL_ROWS: &[LabelRow] = &[
    LabelRow {
        case: "a plain name, the oldest version named",
        written: "plain-labelled",
        addressed: "plain-labelled",
        label: "Production",
        parameter_version: Some(1),
        expected_version: 1,
    },
    LabelRow {
        case: "a hierarchy path, a middle version named",
        written: "/demo/db/password",
        addressed: "/demo/db/password",
        label: "Staging.Candidate",
        parameter_version: Some(2),
        expected_version: 2,
    },
    LabelRow {
        case: "a hierarchy path addressed without its leading slash, no version named",
        written: "/cdk-bootstrap/hnb659fds/version",
        addressed: "cdk-bootstrap/hnb659fds/version",
        label: "latest-known_good",
        parameter_version: None,
        expected_version: 3,
    },
    LabelRow {
        case: "a deep hierarchy path, the newest version named",
        written: "/a/b/c/d/e/release",
        addressed: "/a/b/c/d/e/release",
        label: "v2.1-rc_3",
        parameter_version: Some(3),
        expected_version: 3,
    },
    LabelRow {
        case: "a label of mixed case and digits",
        written: "/mixed/case",
        addressed: "/mixed/case",
        label: "Release2024",
        parameter_version: Some(1),
        expected_version: 1,
    },
];

/// A label attaches to the version the request names — or the newest when it names none
/// — a `name:label` read resolves to that version, and releasing it makes the read miss
/// again.
#[tokio::test]
async fn a_label_attaches_to_the_version_its_request_names() {
    let router = router();

    for row in LABEL_ROWS {
        let case = row.case;
        write_versions(&router, case, row.written, VALUES).await;

        // The longest label the model admits, spelled from the documented character set,
        // is carried alongside every row's own label: one call may attach several.
        let long_label = format!("L{}", "a".repeat(MAX_LABEL_LENGTH - 1));
        let mut request = json!({
            "Name": row.addressed,
            "Labels": [row.label, long_label],
        });
        if let Some(version) = row.parameter_version {
            request["ParameterVersion"] = json!(version);
        }
        let (status, labelled) = call(&router, "LabelParameterVersion", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from LabelParameterVersion, got {status} {labelled}"
        );
        assert_eq!(
            labelled["ParameterVersion"].as_i64(),
            Some(row.expected_version),
            "case {case:?}: expected the labelled version {}, got {labelled}",
            row.expected_version
        );
        assert_eq!(
            labels_of(&labelled, "InvalidLabels"),
            Vec::<String>::new(),
            "case {case:?}: expected no invalid labels for a well-formed label, got {labelled}"
        );

        // The read resolves through the stored label set: the value, version and selector
        // reported are the labelled version's.
        let expected_value = VALUES[(row.expected_version - 1) as usize];
        for label in [row.label.to_string(), long_label.clone()] {
            let selected = format!("{}:{}", row.addressed, label);
            // The read echoes the selector it resolved by in the documented
            // `parameter_name:label` form, separator and all
            // (`smithy_query(query="doc", shape="Parameter", member="Selector")`).
            let echoed = format!(":{label}");
            let (status, read) = call(&router, "GetParameter", &json!({ "Name": selected })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the label read {selected:?} to succeed, got {status} \
                 {read}"
            );
            assert_eq!(
                (
                    read["Parameter"]["Value"].as_str(),
                    read["Parameter"]["Version"].as_i64(),
                    read["Parameter"]["Selector"].as_str(),
                ),
                (
                    Some(expected_value),
                    Some(row.expected_version),
                    Some(echoed.as_str()),
                ),
                "case {case:?}: expected the label to resolve to version {} ({expected_value:?}), \
                 got {read}",
                row.expected_version
            );
        }

        // The history reports the labels on the version that holds them, and on no other.
        let history = history_labels(&router, case, row.addressed).await;
        for (version, held) in &history {
            let expected: Vec<String> = if *version == row.expected_version {
                let mut both = vec![row.label.to_string(), long_label.clone()];
                both.sort();
                both
            } else {
                Vec::new()
            };
            assert_eq!(
                held, &expected,
                "case {case:?}: expected version {version} to report {expected:?}, got {held:?}"
            );
        }

        // Releasing the label takes it off the stored set — and a label that version
        // never held is reported invalid rather than removed.
        let (status, unlabelled) = call(
            &router,
            "UnlabelParameterVersion",
            &json!({
                "Name": row.addressed,
                "ParameterVersion": row.expected_version,
                "Labels": [row.label, "NeverAttached"],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UnlabelParameterVersion, got {status} \
             {unlabelled}"
        );
        assert_eq!(
            (
                labels_of(&unlabelled, "RemovedLabels"),
                labels_of(&unlabelled, "InvalidLabels"),
            ),
            (
                vec![row.label.to_string()],
                vec!["NeverAttached".to_string()],
            ),
            "case {case:?}: expected only the attached label removed, got {unlabelled}"
        );

        // The released label resolves to nothing again, while the one left attached still
        // does: the removal was persisted rather than echoed.
        let (status, missed) = call(
            &router,
            "GetParameter",
            &json!({ "Name": format!("{}:{}", row.addressed, row.label) }),
        )
        .await;
        assert_eq!(
            (status, error_code(&missed).as_str()),
            (400, "ParameterVersionNotFound"),
            "case {case:?}: expected the released label to miss, got {status} {missed}"
        );
        let (status, kept) = call(
            &router,
            "GetParameter",
            &json!({ "Name": format!("{}:{}", row.addressed, long_label) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the label that was not released to still resolve, got \
             {status} {kept}"
        );
    }
}

/// "You can't attach the same label to different versions of the same parameter ... You
/// can move a label from one version of a parameter to another": each attach moves the
/// label, so it is held by exactly one version at a time.
#[tokio::test]
async fn attaching_a_held_label_moves_it_to_the_named_version() {
    let router = router();
    let case = "a label moved along the versions of one parameter";
    let name = "/moved/label";
    let label = "Production";
    write_versions(&router, case, name, VALUES).await;

    // Each step names a version (or none, for the latest) and the version the label must
    // then be held by.
    let steps: &[(Option<i64>, i64)] = &[
        (Some(1), 1),
        (Some(3), 3),
        (None, 3),
        (Some(2), 2),
        (Some(1), 1),
    ];
    for (asked, expected) in steps {
        let mut request = json!({ "Name": name, "Labels": [label] });
        if let Some(version) = asked {
            request["ParameterVersion"] = json!(version);
        }
        let (status, labelled) = call(&router, "LabelParameterVersion", &request).await;
        assert_eq!(
            (status, labelled["ParameterVersion"].as_i64()),
            (200, Some(*expected)),
            "case {case:?}: expected version {expected} labelled for {asked:?}, got {status} \
             {labelled}"
        );

        let (status, read) = call(
            &router,
            "GetParameter",
            &json!({ "Name": format!("{name}:{label}") }),
        )
        .await;
        assert_eq!(
            (
                status,
                read["Parameter"]["Version"].as_i64(),
                read["Parameter"]["Value"].as_str(),
            ),
            (
                200,
                Some(*expected),
                Some(VALUES[(*expected - 1) as usize]),
            ),
            "case {case:?}: expected the moved label to resolve to version {expected}, got \
             {status} {read}"
        );

        // Exactly one version holds it: the move took it off whichever version had it.
        let holders: Vec<i64> = history_labels(&router, case, name)
            .await
            .into_iter()
            .filter(|(_, held)| held.iter().any(|one| one == label))
            .map(|(version, _)| version)
            .collect();
        assert_eq!(
            holders,
            vec![*expected],
            "case {case:?}: expected only version {expected} to hold the label, got {holders:?}"
        );
    }
}

/// One row of the invalid-label suite: a label spelled against one of the documented
/// requirements, and which requirement it breaks.
struct InvalidLabelRow {
    label: &'static str,
    requirement: &'static str,
    /// The error a `name:label` read through that spelling answers with: a label
    /// nothing attached is `ParameterVersionNotFound`, except where the selector makes
    /// the NAME carry a space, which is the name's own spaces rule ("if the specified
    /// name for a parameter contains spaces between characters, the request fails with a
    /// ValidationException error").
    read_code: &'static str,
}

/// Every row breaks one of the requirements `LabelParameterVersion` documents
/// (`smithy_query(query="doc", shape="LabelParameterVersion")`): "labels can contain
/// letters (case sensitive), numbers, periods (.), hyphens (-), or underscores (_).
/// Labels can't begin with a number, "aws" or "ssm" (not case sensitive)".
const INVALID_LABEL_ROWS: &[InvalidLabelRow] = &[
    InvalidLabelRow {
        label: "1production",
        requirement: "begins with a number",
        read_code: "ParameterVersionNotFound",
    },
    InvalidLabelRow {
        label: "9",
        requirement: "is a single digit, so it begins with a number",
        // An all-digit selector names a version rather than a label
        // (`Parameter$Selector`), and no parameter here carries version 9.
        read_code: "ParameterVersionNotFound",
    },
    InvalidLabelRow {
        label: "awsProduction",
        requirement: "begins with \"aws\"",
        read_code: "ParameterVersionNotFound",
    },
    InvalidLabelRow {
        label: "AWS-Production",
        requirement: "begins with \"aws\", which is not case sensitive",
        read_code: "ParameterVersionNotFound",
    },
    InvalidLabelRow {
        label: "ssmProduction",
        requirement: "begins with \"ssm\"",
        read_code: "ParameterVersionNotFound",
    },
    InvalidLabelRow {
        label: "SSM_Production",
        requirement: "begins with \"ssm\", which is not case sensitive",
        read_code: "ParameterVersionNotFound",
    },
    InvalidLabelRow {
        label: "my label",
        requirement: "carries a space, which is not in the label character set",
        read_code: "ValidationException",
    },
    InvalidLabelRow {
        label: "team/label",
        requirement: "carries a slash, which is not in the label character set",
        read_code: "ParameterVersionNotFound",
    },
    InvalidLabelRow {
        label: "label+plus",
        requirement: "carries a plus, which is not in the label character set",
        read_code: "ParameterVersionNotFound",
    },
    InvalidLabelRow {
        label: "étiquette",
        requirement: "carries a non-ASCII letter, which is not in the label character set",
        read_code: "ParameterVersionNotFound",
    },
];

/// A label that fails the documented requirements "isn't associated with a parameter and
/// the system displays it in the list of InvalidLabels" — while the well-formed labels of
/// the same request are attached.
#[tokio::test]
async fn a_label_failing_the_requirements_is_reported_and_attaches_nothing() {
    let router = router();
    let name = "/invalid/labels";
    write_versions(
        &router,
        "the parameter every invalid-label row labels",
        name,
        VALUES,
    )
    .await;

    for (index, row) in INVALID_LABEL_ROWS.iter().enumerate() {
        let case = row.requirement;
        // A well-formed companion in the same call, so the row proves the invalid label
        // is reported rather than the whole call refused.
        let companion = format!("Accepted-{index}");
        let (status, labelled) = call(
            &router,
            "LabelParameterVersion",
            &json!({
                "Name": name,
                "ParameterVersion": 1,
                "Labels": [row.label, companion],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 with the label reported invalid, got {status} \
             {labelled}"
        );
        assert_eq!(
            labels_of(&labelled, "InvalidLabels"),
            vec![row.label.to_string()],
            "case {case:?}: expected {:?} reported invalid, got {labelled}",
            row.label
        );

        // Nothing was attached for it: the read through that selector misses, while the
        // companion resolves.
        let (status, missed) = call(
            &router,
            "GetParameter",
            &json!({ "Name": format!("{name}:{}", row.label) }),
        )
        .await;
        assert_eq!(
            (status, error_code(&missed).as_str()),
            (400, row.read_code),
            "case {case:?}: expected a read through the rejected label to answer {}, got {status} \
             {missed}",
            row.read_code
        );
        let (status, read) = call(
            &router,
            "GetParameter",
            &json!({ "Name": format!("{name}:{companion}") }),
        )
        .await;
        assert_eq!(
            (status, read["Parameter"]["Version"].as_i64()),
            (200, Some(1)),
            "case {case:?}: expected the well-formed companion label to resolve, got {status} \
             {read}"
        );

        // An invalid label is attached to nothing, so releasing it reports it unattached.
        let (status, unlabelled) = call(
            &router,
            "UnlabelParameterVersion",
            &json!({
                "Name": name,
                "ParameterVersion": 1,
                "Labels": [row.label],
            }),
        )
        .await;
        assert_eq!(
            (status, labels_of(&unlabelled, "RemovedLabels")),
            (200, Vec::new()),
            "case {case:?}: expected nothing removed for a label never attached, got {status} \
             {unlabelled}"
        );
    }
}

/// "A version of a parameter can have a maximum of 10 labels": the bound holds at the
/// published maximum and refuses the call that would carry a version past it, while
/// another version of the same parameter still takes its own.
#[tokio::test]
async fn a_version_holds_at_most_the_published_number_of_labels() {
    let router = router();
    let case = "the per-version label maximum";
    let name = "/labels/limit";
    write_versions(&router, case, name, VALUES).await;

    // Exactly the maximum, in one call: `ParameterLabelList` carries up to 10 labels.
    let full: Vec<String> = (0..MAX_LABELS_PER_VERSION)
        .map(|index| format!("Label-{index}"))
        .collect();
    let (status, labelled) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": name, "ParameterVersion": 1, "Labels": full }),
    )
    .await;
    assert_eq!(
        (status, labels_of(&labelled, "InvalidLabels")),
        (200, Vec::new()),
        "case {case:?}: expected {MAX_LABELS_PER_VERSION} labels to attach, got {status} \
         {labelled}"
    );

    // One past the bound is the modelled refusal, and the version keeps the labels it had.
    let (status, refused) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": name, "ParameterVersion": 1, "Labels": ["OneTooMany"] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ParameterVersionLabelLimitExceeded"),
        "case {case:?}: expected the label past the maximum to be refused, got {status} {refused}"
    );

    // Re-attaching a label the version already holds is not a newcomer, so it stays
    // inside the bound.
    let (status, again) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": name, "ParameterVersion": 1, "Labels": [full[0].clone()] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected re-attaching a held label to succeed, got {status} {again}"
    );

    let held = held_labels(&router, case, name, 1).await;
    assert_eq!(
        held.len(),
        MAX_LABELS_PER_VERSION,
        "case {case:?}: expected the version to still hold {MAX_LABELS_PER_VERSION} labels, got \
         {held:?}"
    );
    assert!(
        !held.iter().any(|label| label == "OneTooMany"),
        "case {case:?}: expected the refused label not to be attached, got {held:?}"
    );

    // The bound is per version: another version of the same parameter takes its own
    // labels, and a label moved out of the full version makes room in it.
    let (status, other) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": name, "ParameterVersion": 2, "Labels": [full[0].clone(), "Spare"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected another version to take its own labels, got {status} {other}"
    );
    assert_eq!(
        held_labels(&router, case, name, 1).await.len(),
        MAX_LABELS_PER_VERSION - 1,
        "case {case:?}: expected the moved label to leave the version it was held by"
    );
    let (status, room) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": name, "ParameterVersion": 1, "Labels": ["RoomMade"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the freed slot to take a new label, got {status} {room}"
    );
}

/// One row of the refusal suite: how a label call addresses a parameter, and the error
/// code that addressing must answer with.
struct RefusalRow {
    case: &'static str,
    /// The `Name` the call carries, built per row because some rows carry an ARN.
    name: String,
    parameter_version: Option<i64>,
    expected_code: &'static str,
}

/// A name the store holds no parameter for is `ParameterNotFound`, and a version the
/// parameter does not carry is `ParameterVersionNotFound` — for both label operations.
#[tokio::test]
async fn label_calls_refuse_a_name_or_version_the_store_does_not_hold() {
    let router = router();
    let name = "/refusals/parameter";
    write_versions(&router, "the parameter the refusal rows address", name, VALUES).await;

    let rows = vec![
        RefusalRow {
            case: "a name nothing created",
            name: "/refusals/absent".to_string(),
            parameter_version: Some(1),
            expected_code: "ParameterNotFound",
        },
        RefusalRow {
            case: "a name carrying a label selector",
            name: format!("{name}:Production"),
            parameter_version: Some(1),
            expected_code: "ParameterNotFound",
        },
        RefusalRow {
            case: "a full ARN rather than the parameter name itself",
            name: format!("arn:aws:ssm:{REGION}:{ACCOUNT}:parameter{name}"),
            parameter_version: Some(1),
            expected_code: "ParameterNotFound",
        },
        RefusalRow {
            case: "a version past the newest the parameter carries",
            name: name.to_string(),
            parameter_version: Some((VALUES.len() + 1) as i64),
            expected_code: "ParameterVersionNotFound",
        },
        RefusalRow {
            case: "a version number no parameter counts from",
            name: name.to_string(),
            parameter_version: Some(0),
            expected_code: "ParameterVersionNotFound",
        },
        RefusalRow {
            case: "a negative version number",
            name: name.to_string(),
            parameter_version: Some(-1),
            expected_code: "ParameterVersionNotFound",
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut label_request = json!({ "Name": row.name, "Labels": ["Production"] });
        if let Some(version) = row.parameter_version {
            label_request["ParameterVersion"] = json!(version);
        }
        let (status, refused) = call(&router, "LabelParameterVersion", &label_request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, row.expected_code),
            "case {case:?}: expected 400 {} from LabelParameterVersion, got {status} {refused}",
            row.expected_code
        );

        // `UnlabelParameterVersion` requires the version, so every row names one.
        let (status, refused) = call(
            &router,
            "UnlabelParameterVersion",
            &json!({
                "Name": row.name,
                "ParameterVersion": row.parameter_version.unwrap_or(1),
                "Labels": ["Production"],
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, row.expected_code),
            "case {case:?}: expected 400 {} from UnlabelParameterVersion, got {status} {refused}",
            row.expected_code
        );
    }

    // A parameter whose name carries spaces between characters is the spaces rule, not a
    // not-found: "the request fails with a ValidationException error".
    let (status, refused) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": "/spaced name/param", "Labels": ["Production"] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"a name with spaces between characters\": expected 400 ValidationException, got \
         {status} {refused}"
    );
}

/// One row of the batch-delete suite: what a name names, and which list it belongs in.
struct DeleteRow {
    case: &'static str,
    /// The name the batch delete carries.
    asked: String,
    /// Whether the store held a parameter that name removes.
    deleted: bool,
}

/// `DeleteParameters` partitions the names it was given by what the store held: every
/// name whose parameter it removed is reported deleted, every other name invalid — and
/// the removals are persisted, so a later read of a deleted name misses.
#[tokio::test]
async fn a_batch_delete_partitions_its_names_by_what_the_store_held() {
    let router = router();
    let case = "a batch delete over names of every form";

    // Written before the delete: each of these is a parameter the store holds.
    let stored = [
        "plain-batched",
        "/batch/db/password",
        "/batch/deep/a/b/c/value",
        "/batch/kept",
    ];
    for name in stored {
        write_versions(&router, case, name, &["only"]).await;
    }
    // One of them carries a label, so the delete removes a labelled parameter too.
    let (status, labelled) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": "/batch/db/password", "Labels": ["Production"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the label to attach before the delete, got {status} {labelled}"
    );

    let rows = [
        DeleteRow {
            case: "a plain name the store holds",
            asked: "plain-batched".to_string(),
            deleted: true,
        },
        DeleteRow {
            case: "a hierarchy path the store holds",
            asked: "/batch/db/password".to_string(),
            deleted: true,
        },
        DeleteRow {
            case: "a held path addressed without its leading slash",
            asked: "batch/deep/a/b/c/value".to_string(),
            deleted: true,
        },
        DeleteRow {
            case: "a name nothing created",
            asked: "/batch/absent".to_string(),
            deleted: false,
        },
        DeleteRow {
            case: "a name already removed by this very call",
            asked: "/plain-batched".to_string(),
            deleted: false,
        },
        DeleteRow {
            case: "a name carrying a version selector",
            asked: "/batch/kept:1".to_string(),
            deleted: false,
        },
        DeleteRow {
            case: "a full ARN rather than the parameter name itself",
            asked: format!("arn:aws:ssm:{REGION}:{ACCOUNT}:parameter/batch/kept"),
            deleted: false,
        },
    ];


    let (status, response) = call(
        &router,
        "DeleteParameters",
        &json!({ "Names": rows.iter().map(|row| row.asked.clone()).collect::<Vec<String>>() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteParameters, got {status} {response}"
    );
    let expected_deleted: Vec<String> = rows
        .iter()
        .filter(|row| row.deleted)
        .map(|row| row.asked.clone())
        .collect();
    let expected_invalid: Vec<String> = rows
        .iter()
        .filter(|row| !row.deleted)
        .map(|row| row.asked.clone())
        .collect();
    assert_eq!(
        labels_of(&response, "DeletedParameters"),
        expected_deleted,
        "case {case:?}: expected {expected_deleted:?} reported deleted, got {response}"
    );
    assert_eq!(
        labels_of(&response, "InvalidParameters"),
        expected_invalid,
        "case {case:?}: expected {expected_invalid:?} reported invalid, got {response}"
    );

    // What was reported deleted is gone from the store, including through a label that
    // pointed at one of its versions; what was not is still there.
    for row in rows.iter().filter(|row| row.deleted) {
        let (status, gone) = call(&router, "GetParameter", &json!({ "Name": row.asked })).await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (400, "ParameterNotFound"),
            "case {:?}: expected the deleted name to miss, got {status} {gone}",
            row.case
        );
    }
    let (status, gone) = call(
        &router,
        "GetParameter",
        &json!({ "Name": "/batch/db/password:Production" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&gone).as_str()),
        (400, "ParameterNotFound"),
        "case {case:?}: expected a label of a deleted parameter to miss, got {status} {gone}"
    );
    let (status, kept) = call(&router, "GetParameter", &json!({ "Name": "/batch/kept" })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the parameter no name removed to survive, got {status} {kept}"
    );

    // A batch delete reaches only the Region it is signed for: another Region holds no
    // parameter of that name, so the name is invalid there and the parameter survives.
    let other = "eu-central-1";
    let (status, elsewhere) = call_in_region(
        &router,
        other,
        "DeleteParameters",
        &json!({ "Names": ["/batch/kept"] }),
    )
    .await;
    assert_eq!(
        (
            status,
            labels_of(&elsewhere, "DeletedParameters"),
            labels_of(&elsewhere, "InvalidParameters"),
        ),
        (200, Vec::new(), vec!["/batch/kept".to_string()]),
        "case {case:?}: expected the delete in {other} to report the name invalid, got {status} \
         {elsewhere}"
    );
    let (status, survived) = call(&router, "GetParameter", &json!({ "Name": "/batch/kept" })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other Region's parameter to survive, got {status} {survived}"
    );

    // The names one call carries are a set: a name given twice names the one parameter,
    // so it is reported once rather than both deleted and invalid.
    let repeated = "/batch/repeated";
    write_versions(&router, case, repeated, &["only"]).await;
    let (status, twice) = call(
        &router,
        "DeleteParameters",
        &json!({ "Names": [repeated, repeated] }),
    )
    .await;
    assert_eq!(
        (
            status,
            labels_of(&twice, "DeletedParameters"),
            labels_of(&twice, "InvalidParameters"),
        ),
        (200, vec![repeated.to_string()], Vec::new()),
        "case \"one name given twice\": expected it reported deleted once, got {status} {twice}"
    );
}

/// Every read that resolves a label observes the stored label sets without any change of
/// its own: the batch read resolves a `name:label` selector it holds and reports one it
/// does not among the invalid names, and a hierarchy read filtered on `Label` reports the
/// labelled version rather than the newest.
#[tokio::test]
async fn the_label_reads_resolve_through_the_stored_label_sets() {
    let router = router();
    let case = "the reads that resolve a label";
    let name = "/observed/parameter";
    write_versions(&router, case, name, VALUES).await;

    let label = "Release.1";
    let (status, labelled) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": name, "ParameterVersion": 1, "Labels": [label] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the label to attach, got {status} {labelled}"
    );

    // The batch read: a selector the parameter holds resolves, one it does not is
    // reported invalid.
    let (status, batch) = call(
        &router,
        "GetParameters",
        &json!({ "Names": [format!("{name}:{label}"), format!("{name}:Absent")] }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: expected HTTP 200, got {batch}");
    assert_eq!(
        batch["Parameters"]
            .as_array()
            .map(|entries| entries
                .iter()
                .map(|entry| (entry["Version"].as_i64(), entry["Value"].as_str()))
                .collect::<Vec<_>>()),
        Some(vec![(Some(1), Some(VALUES[0]))]),
        "case {case:?}: expected only the labelled version reported, got {batch}"
    );
    assert_eq!(
        labels_of(&batch, "InvalidParameters"),
        vec![format!("{name}:Absent")],
        "case {case:?}: expected the unheld selector reported invalid, got {batch}"
    );

    // The hierarchy read: "a label can be assigned to only one version of a parameter at
    // a time", so a `Label` filter reports the version holding it.
    let (status, by_path) = call(
        &router,
        "GetParametersByPath",
        &json!({
            "Path": "/observed",
            "ParameterFilters": [{ "Key": "Label", "Option": "Equals", "Values": [label] }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetParametersByPath, got {by_path}"
    );
    assert_eq!(
        by_path["Parameters"]
            .as_array()
            .map(|entries| entries
                .iter()
                .map(|entry| (entry["Name"].as_str(), entry["Version"].as_i64()))
                .collect::<Vec<_>>()),
        Some(vec![(Some(name), Some(1))]),
        "case {case:?}: expected the labelled version reported under the path, got {by_path}"
    );

    // Released, the same filter reports nothing: both reads follow the stored set.
    let (status, unlabelled) = call(
        &router,
        "UnlabelParameterVersion",
        &json!({ "Name": name, "ParameterVersion": 1, "Labels": [label] }),
    )
    .await;
    assert_eq!(
        (status, labels_of(&unlabelled, "RemovedLabels")),
        (200, vec![label.to_string()]),
        "case {case:?}: expected the label released, got {status} {unlabelled}"
    );
    let (status, empty) = call(
        &router,
        "GetParametersByPath",
        &json!({
            "Path": "/observed",
            "ParameterFilters": [{ "Key": "Label", "Option": "Equals", "Values": [label] }],
        }),
    )
    .await;
    assert_eq!(
        (status, empty["Parameters"].as_array().map(Vec::len)),
        (200, Some(0)),
        "case {case:?}: expected the released label to match nothing, got {status} {empty}"
    );
}

/// `PutParameter` once per value, so the parameter carries one version per value.
async fn write_versions(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    values: &[&str],
) {
    for (index, value) in values.iter().enumerate() {
        let mut request = json!({ "Name": name, "Type": "String", "Value": value });
        if index > 0 {
            request["Overwrite"] = Value::Bool(true);
        }
        let (status, response) = call(router, "PutParameter", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutParameter of {name:?} to succeed, got {status} {response}"
        );
    }
}

/// The string list `member` carries, as a `Vec<String>`; an empty list for a member the
/// response does not carry.
fn labels_of(response: &Value, member: &str) -> Vec<String> {
    response[member]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every version of `name` and the labels `GetParameterHistory` reports on it, oldest
/// first — the stored label sets, read back through an implemented reader.
async fn history_labels(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> Vec<(i64, Vec<String>)> {
    let (status, history) = call(router, "GetParameterHistory", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetParameterHistory of {name:?} to succeed, got {status} \
         {history}"
    );
    history["Parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the history carries a Parameters list: {history}"))
        .iter()
        .map(|entry| {
            let version = entry["Version"].as_i64().unwrap_or_else(|| {
                panic!("case {case:?}: every history entry carries a Version: {entry}")
            });
            (version, labels_of(entry, "Labels"))
        })
        .collect()
}

/// The labels the history reports on one version of `name`.
async fn held_labels(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    version: i64,
) -> Vec<String> {
    history_labels(router, case, name)
        .await
        .into_iter()
        .find(|(held, _)| *held == version)
        .map(|(_, labels)| labels)
        .unwrap_or_else(|| panic!("case {case:?}: the history reports version {version} of {name}"))
}
