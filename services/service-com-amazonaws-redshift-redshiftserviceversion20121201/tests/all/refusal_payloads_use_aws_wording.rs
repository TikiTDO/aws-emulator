// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE four families of refusal carry, end to end over the awsQuery
//! wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The RULE every case asserts: a refusal's message is Amazon Redshift's own
//! wording for that error CLASS — its fixed text exactly, with the variable
//! parts taken from the request and from the emulator's own catalog — and never
//! a shared "<Type> <condition>: <id>" template, a Smithy documentation sentence
//! or a wire location. Concretely:
//!
//! * a parameter value the published `AllowedValues` set does not admit is
//!   refused in the vocabulary of the PARAMETER: by its form, naming the
//!   parameter's data type and the value, or by its magnitude, naming the
//!   parameter, the value and the published range — never naming
//!   `Parameters.Parameter.<n>.ParameterValue`;
//! * `ModifyClusterParameterGroup` on a group that does not exist answers the
//!   fixed sentence that names NO group, while the group listing answers its own
//!   sentence for the same name;
//! * a duplicate identifier is refused per RESOURCE: the cluster sentence
//!   carries no identifier at all, the snapshot sentence carries one;
//! * a tagging `ResourceName` that resolves to nothing is refused naming the
//!   resource TYPE the ARN carries and the bare identifier after it, so a
//!   missing cluster and a missing parameter group do not read alike.
//!
//! The expected sentences are written out literally here, so a change to the
//! fixed text fails a test rather than passing one computed from the code under
//! test. The rows differ along what each rule generalises over: the parameter's
//! data type and which published bound a value misses, the shape of an absent
//! name, which operation reports the refusal, and the resource type an ARN
//! names.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator, REGION};
use crate::cluster_parameters::create_group;
use crate::copy_cluster_snapshot::seed_automated;
use crate::modify_cluster_parameter_group::modify;

/// Assert the status, awsQuery `<Code>` and `<Message>` of one refusal.
fn assert_refusal(case: &str, answer: &Answer, status: u16, code: &str, message: &str) {
    assert_eq!(
        (answer.status, answer.error_code(), answer.text("Message")),
        (
            status,
            Some(code.to_string()),
            Some(message.to_string())
        ),
        "case {case}: expected {status} {code} {message:?}, got {} / {:?} / {:?} with body {}",
        answer.status,
        answer.error_code(),
        answer.text("Message"),
        answer.body
    );
}

/// One row of the atomicity table: the case, the name-value pairs one request
/// carries and the whole sentence the refusal must carry.
type BatchRefusal<'a> = (&'a str, Vec<(&'a str, &'a str)>, &'a str);

/// One row of the parameter-value table: the parameter the request sets, the
/// value it sends and the whole sentence the refusal must carry.
struct ValueRefusal {
    case: &'static str,
    parameter: &'static str,
    value: &'static str,
    message: &'static str,
}

/// A value the published set does not admit is refused in the vocabulary of the
/// parameter, by its form or by its magnitude.
///
/// Source for the parameters, their data types and their ranges: the
/// `AllowedValues` and `DataType` of `state::ENGINE_DEFAULT_PARAMETERS`, which
/// is what `smithy_query(query="doc", shape="Parameter",
/// member="AllowedValues")` ("The valid range of values for the parameter") and
/// `member="DataType"` publish per parameter. A set is a comma-separated list of
/// literals (`true,false`) or inclusive numeric ranges (`-15-2`, `0-10`,
/// `0,100-2147483647`), so each published range is covered one past BOTH of its
/// bounds, and the table holds parameters the issue did not cite
/// (`extra_float_digits`, `statement_timeout`, `auto_mv`, `use_fips_ssl`).
#[tokio::test]
async fn a_parameter_value_refusal_is_worded_from_the_parameter_not_from_the_wire() {
    let emulator = Emulator::new();
    const GROUP: &str = "value-refusal-wording";
    create_group(&emulator, GROUP).await;

    let rows = [
        // A boolean parameter: anything but a published literal is refused on
        // its form, whatever shape the value has.
        ValueRefusal {
            case: "a word for a boolean",
            parameter: "require_ssl",
            value: "maybe",
            message: "parameter value is not a valid boolean:maybe",
        },
        ValueRefusal {
            case: "a published literal in the wrong case for a boolean",
            parameter: "auto_mv",
            value: "TRUE",
            message: "parameter value is not a valid boolean:TRUE",
        },
        ValueRefusal {
            case: "a number for a boolean",
            parameter: "use_fips_ssl",
            value: "1",
            message: "parameter value is not a valid boolean:1",
        },
        // An integer parameter: a value that is no whole number is refused on
        // its form, in the same shape of sentence as a boolean's.
        ValueRefusal {
            case: "a word for an integer",
            parameter: "max_concurrency_scaling_clusters",
            value: "many",
            message: "parameter value is not a valid integer:many",
        },
        ValueRefusal {
            case: "a fraction for an integer",
            parameter: "extra_float_digits",
            value: "1.5",
            message: "parameter value is not a valid integer:1.5",
        },
        // A whole number the published range does not contain is refused on its
        // magnitude, naming the parameter, the value and the range.
        ValueRefusal {
            case: "one above the upper bound of 0-10",
            parameter: "max_concurrency_scaling_clusters",
            value: "11",
            message: "The max_concurrency_scaling_clusters of 11 is outside of the acceptable range of 0-10",
        },
        ValueRefusal {
            case: "one below the lower bound of 0-10",
            parameter: "max_concurrency_scaling_clusters",
            value: "-1",
            message: "The max_concurrency_scaling_clusters of -1 is outside of the acceptable range of 0-10",
        },
        ValueRefusal {
            case: "far above the upper bound of 0-10",
            parameter: "max_concurrency_scaling_clusters",
            value: "4096",
            message: "The max_concurrency_scaling_clusters of 4096 is outside of the acceptable range of 0-10",
        },
        ValueRefusal {
            case: "one above the upper bound of a range with a negative low",
            parameter: "extra_float_digits",
            value: "3",
            message: "The extra_float_digits of 3 is outside of the acceptable range of -15-2",
        },
        ValueRefusal {
            case: "one below the lower bound of a range with a negative low",
            parameter: "extra_float_digits",
            value: "-16",
            message: "The extra_float_digits of -16 is outside of the acceptable range of -15-2",
        },
        ValueRefusal {
            case: "between the literal and the range of 0,100-2147483647",
            parameter: "statement_timeout",
            value: "50",
            message:
                "The statement_timeout of 50 is outside of the acceptable range of 0-2147483647",
        },
        ValueRefusal {
            case: "one above the upper bound of 0,100-2147483647",
            parameter: "statement_timeout",
            value: "2147483648",
            message: "The statement_timeout of 2147483648 is outside of the acceptable range of 0-2147483647",
        },
    ];

    for row in &rows {
        let answer = modify(&emulator, GROUP, &[(row.parameter, row.value)]).await;
        assert_refusal(
            row.case,
            &answer,
            400,
            "InvalidParameterValue",
            row.message,
        );
        // The sentence is about the parameter, not about where the value sat in
        // the request: the query serialization index never appears in it.
        let message = answer.text("Message").unwrap_or_default();
        assert!(
            !message.contains("Parameters.Parameter"),
            "case {}: expected no wire location in the refusal, got {message:?}",
            row.case
        );
    }
}

/// A batch carrying one value the service refuses applies NONE of its values,
/// and leaves an override a previous request set exactly as it was.
#[tokio::test]
async fn a_refused_parameter_batch_applies_none_of_the_values_it_carries() {
    let emulator = Emulator::new();
    const GROUP: &str = "atomic-parameter-batch";
    create_group(&emulator, GROUP).await;

    let accepted = modify(&emulator, GROUP, &[("require_ssl", "true")]).await;
    assert_eq!(
        accepted.status, 200,
        "case seed one override: expected 200, got {} with body {}",
        accepted.status, accepted.body
    );

    // Each row carries one value the service takes and one it refuses, in both
    // orders and for both refusal classes, so neither position nor class lets a
    // partial write through.
    let rows: [BatchRefusal; 4] = [
        (
            "the refused value last",
            vec![("auto_mv", "false"), ("require_ssl", "maybe")],
            "parameter value is not a valid boolean:maybe",
        ),
        (
            "the refused value first",
            vec![("require_ssl", "maybe"), ("auto_mv", "false")],
            "parameter value is not a valid boolean:maybe",
        ),
        (
            "an out-of-range value last",
            vec![
                ("auto_mv", "false"),
                ("max_concurrency_scaling_clusters", "11"),
            ],
            "The max_concurrency_scaling_clusters of 11 is outside of the acceptable range of 0-10",
        ),
        (
            "an out-of-range value first",
            vec![
                ("max_concurrency_scaling_clusters", "11"),
                ("auto_mv", "false"),
            ],
            "The max_concurrency_scaling_clusters of 11 is outside of the acceptable range of 0-10",
        ),
    ];

    for (case, sent, message) in rows {
        let refused = modify(&emulator, GROUP, &sent).await;
        assert_refusal(case, &refused, 400, "InvalidParameterValue", message);

        // Only the override the accepted request set is reported with `Source`
        // `user`: the valid member of the refused batch was not applied.
        let user_values: Vec<(String, String)> =
            crate::modify_cluster_parameter_group::reported(&emulator, GROUP)
                .await
                .into_iter()
                .filter(|(_, _, source)| source == state::PARAMETER_SOURCE_USER)
                .map(|(name, value, _)| (name, value))
                .collect();
        assert_eq!(
            user_values,
            vec![("require_ssl".to_string(), "true".to_string())],
            "case {case}: expected the refused batch to change nothing, got {user_values:?}"
        );
    }
}

/// The parameter-group MODIFY says the group does not exist, with no variable
/// part; the group LISTING answers its own sentence for the same name.
#[tokio::test]
async fn the_parameter_group_modify_and_listing_word_one_missing_group_differently() {
    let emulator = Emulator::new();

    // A group that DOES exist, so a refusal below cannot be an "everything is
    // refused" artefact, and one created then deleted: a name this account held
    // resolves to nothing once it is gone.
    create_group(&emulator, "wording-present").await;
    create_group(&emulator, "wording-doomed").await;
    let removed = emulator
        .call(
            "DeleteClusterParameterGroup",
            &[("ParameterGroupName", "wording-doomed")],
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "case delete the doomed group: expected 200, got {} with body {}",
        removed.status, removed.body
    );

    // Rows differing along HOW the name misses: nothing resembles it, a prefix
    // of a stored one, one extra character, and one this account deleted.
    for (case, name) in [
        ("a name nothing created", "wording-absent"),
        ("a prefix of a stored name", "wording-prese"),
        ("a stored name with one extra character", "wording-presentx"),
        ("a group this account deleted", "wording-doomed"),
    ] {
        let modified = modify(&emulator, name, &[("require_ssl", "true")]).await;
        assert_refusal(
            case,
            &modified,
            404,
            "ClusterParameterGroupNotFound",
            "Parameter group does not exist",
        );

        let listed = emulator
            .call(
                "DescribeClusterParameterGroups",
                &[("ParameterGroupName", name)],
            )
            .await;
        assert_refusal(
            case,
            &listed,
            404,
            "ClusterParameterGroupNotFound",
            &format!("ClusterParameterGroup {name} not found."),
        );
    }

    // The group that exists is still there, and still takes a value: not one
    // refusal removed or froze it.
    let still_writable = modify(&emulator, "wording-present", &[("require_ssl", "true")]).await;
    assert_eq!(
        still_writable.status, 200,
        "case the surviving group: expected 200, got {} with body {}",
        still_writable.status, still_writable.body
    );
}

/// `CreateCluster` for one identifier, with just the three required members.
async fn create_cluster(emulator: &Emulator, identifier: &str) -> Answer {
    emulator.create_minimal(identifier).await
}

/// `CreateClusterSnapshot` of `cluster` under `identifier`.
async fn create_snapshot(emulator: &Emulator, identifier: &str, cluster: &str) -> Answer {
    emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", identifier),
                ("ClusterIdentifier", cluster),
            ],
        )
        .await
}

/// A duplicate identifier is refused with the sentence of ITS OWN resource: the
/// cluster sentence names no identifier, the snapshot sentence names one.
#[tokio::test]
async fn a_duplicate_identifier_is_refused_with_the_sentence_of_its_own_resource() {
    let emulator = Emulator::new();

    // Rows differing along the shape of the identifier the account already
    // holds: a short name, a long hyphenated one, and one carrying digits.
    for (case, identifier) in [
        ("a short identifier", "dup-a"),
        (
            "a long hyphenated identifier",
            "dup-cluster-with-several-hyphens",
        ),
        ("an identifier carrying digits", "dup-cluster-2026"),
    ] {
        let created = create_cluster(&emulator, identifier).await;
        assert_eq!(
            created.status, 200,
            "case {case}: the first create must be served, got {} with body {}",
            created.status, created.body
        );

        // The cluster sentence has NO variable part: the same text whatever
        // identifier the request re-used.
        let duplicate = create_cluster(&emulator, identifier).await;
        assert_refusal(
            case,
            &duplicate,
            400,
            "ClusterAlreadyExists",
            "Cluster already exists",
        );
        assert!(
            !duplicate
                .text("Message")
                .unwrap_or_default()
                .contains(identifier),
            "case {case}: expected the cluster sentence to name no identifier, got {:?}",
            duplicate.text("Message")
        );

        // The snapshot sentence DOES carry the identifier the request asked for.
        let snapshot = format!("{identifier}-snap");
        let taken = create_snapshot(&emulator, &snapshot, identifier).await;
        assert_eq!(
            taken.status, 200,
            "case {case}: the first snapshot must be served, got {} with body {}",
            taken.status, taken.body
        );
        let repeat = create_snapshot(&emulator, &snapshot, identifier).await;
        assert_refusal(
            case,
            &repeat,
            400,
            "ClusterSnapshotAlreadyExists",
            &format!(
                "Cannot create the snapshot because a snapshot with the identifier {snapshot} already exists."
            ),
        );

        // The same snapshot sentence answers the COPY that would take the
        // identifier, which mints a snapshot under a caller-chosen name too. Its
        // source has to be a snapshot Amazon Redshift took itself ("Only
        // automated snapshots can be copied"), so one is seeded into the store
        // the handlers read.
        let source = format!("{identifier}-auto");
        seed_automated(&emulator, identifier, &source, state::STATUS_AVAILABLE);
        let copied = emulator
            .call(
                "CopyClusterSnapshot",
                &[
                    ("SourceSnapshotIdentifier", &source),
                    ("TargetSnapshotIdentifier", &snapshot),
                ],
            )
            .await;
        assert_refusal(
            case,
            &copied,
            400,
            "ClusterSnapshotAlreadyExists",
            &format!(
                "Cannot create the snapshot because a snapshot with the identifier {snapshot} already exists."
            ),
        );
    }

    // A rename onto a cluster the account already holds answers the cluster
    // sentence as well: the fault is the resource's, not the operation's.
    let renamed = emulator
        .call(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", "dup-a"),
                ("NewClusterIdentifier", "dup-cluster-2026"),
            ],
        )
        .await;
    assert_refusal(
        "a rename onto an existing cluster",
        &renamed,
        400,
        "ClusterAlreadyExists",
        "Cluster already exists",
    );
}

/// `CreateTags` with one tag on `resource_name`.
async fn tag(emulator: &Emulator, resource_name: &str) -> Answer {
    emulator
        .call(
            "CreateTags",
            &[
                ("ResourceName", resource_name),
                ("Tags.Tag.1.Key", "owner"),
                ("Tags.Tag.1.Value", "admin"),
            ],
        )
        .await
}

/// A `ResourceName` that resolves to nothing is refused naming the resource type
/// the ARN carries and the bare identifier after it.
///
/// Source for the per-type ARN segments: the Amazon Redshift Cluster Management
/// Guide's "Constructing an Amazon Redshift Amazon Resource Name (ARN)", the
/// page `smithy_query(query="doc", shape="TaggedResource",
/// member="ResourceType")` links to — the same segments
/// `state::TAGGABLE_RESOURCE_TYPES` publishes. The table holds types the issue
/// did not cite (snapshot, parameter group, subnet group) and the two values
/// that name no type at all.
#[tokio::test]
async fn a_tagging_arn_that_names_nothing_reports_the_type_and_the_identifier() {
    let emulator = Emulator::new();

    // One resource that DOES exist, tagged, so the refusals below cannot be an
    // "everything is refused" artefact and the positive control is observable.
    create_group(&emulator, "tagged-group").await;
    let present = state::parameter_group_arn("tagged-group", REGION);
    assert_eq!(
        tag(&emulator, &present).await.status,
        200,
        "case seed one tag: expected 200"
    );

    let rows: Vec<(&str, String, String)> = vec![
        (
            "a cluster that does not exist",
            state::cluster_arn("absent-cluster", REGION),
            "cluster (absent-cluster) was not found.".to_string(),
        ),
        (
            "a snapshot that does not exist",
            state::snapshot_arn("absent-cluster", "absent-snap", REGION),
            "snapshot (absent-cluster/absent-snap) was not found.".to_string(),
        ),
        (
            "a parameter group that does not exist",
            state::parameter_group_arn("absent-group", REGION),
            "parametergroup (absent-group) was not found.".to_string(),
        ),
        (
            "a subnet group that does not exist",
            state::subnet_group_arn("absent-subnet-group", REGION),
            "subnetgroup (absent-subnet-group) was not found.".to_string(),
        ),
        (
            "a published type this account holds none of",
            engine::emulator_arn("redshift", REGION, "snapshotcopygrant:absent-grant"),
            "snapshotcopygrant (absent-grant) was not found.".to_string(),
        ),
        (
            "a stored resource's type with another identifier",
            state::parameter_group_arn("tagged-group-x", REGION),
            "parametergroup (tagged-group-x) was not found.".to_string(),
        ),
        // Two values that name no resource TYPE: there is no type to report, so
        // the refusal names the value the request carried instead.
        (
            "a Redshift ARN with an unpublished resource type",
            engine::emulator_arn("redshift", REGION, "widget:absent-widget"),
            format!(
                "The resource could not be found: {}",
                engine::emulator_arn("redshift", REGION, "widget:absent-widget")
            ),
        ),
        (
            "not an ARN at all",
            "absent-cluster".to_string(),
            "The resource could not be found: absent-cluster".to_string(),
        ),
    ];

    for (case, resource_name, message) in &rows {
        for action in ["CreateTags", "DeleteTags", "DescribeTags"] {
            let case = format!("{case} on {action}");
            let answer = match action {
                "CreateTags" => tag(&emulator, resource_name).await,
                "DeleteTags" => {
                    emulator
                        .call(
                            "DeleteTags",
                            &[
                                ("ResourceName", resource_name),
                                ("TagKeys.TagKey.1", "owner"),
                            ],
                        )
                        .await
                }
                _ => {
                    emulator
                        .call("DescribeTags", &[("ResourceName", resource_name)])
                        .await
                }
            };
            assert_refusal(&case, &answer, 404, "ResourceNotFoundFault", message);
        }
    }

    // The resource that exists keeps the tag every refusal left alone.
    let reported = emulator
        .call("DescribeTags", &[("ResourceName", &present)])
        .await;
    assert_eq!(
        (reported.status, reported.text("Key"), reported.text("Value")),
        (200, Some("owner".to_string()), Some("admin".to_string())),
        "case the surviving tag: got {} / {:?} / {:?} with body {}",
        reported.status,
        reported.text("Key"),
        reported.text("Value"),
        reported.body
    );
}
