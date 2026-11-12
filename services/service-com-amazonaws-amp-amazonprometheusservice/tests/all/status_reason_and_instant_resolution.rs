// SPDX-License-Identifier: CC0-1.0

//! What this service PUBLISHES on a rule groups namespace and an alert manager
//! definition: the status structure's `statusReason`, and the resolution of the instants
//! beside it.
//!
//! Two rules are under test, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! 1. A read path reports the status structure's `statusReason` — the reason the stored
//!    record holds, or the service's no-failure reason
//!    (`state::NO_FAILURE_STATUS_REASON`) while it holds none — so the modelled member is
//!    there to be read instead of missing from the envelope. Asserted on
//!    `DescribeRuleGroupsNamespace`, `ListRuleGroupsNamespaces` and
//!    `DescribeAlertManagerDefinition`, in each lifecycle status a record reaches, and
//!    against reasons seeded onto the record itself.
//! 2. Every instant this service publishes carries at most MILLISECOND resolution — the
//!    resolution of the epoch-seconds number `restJson1` writes a `Timestamp` as — and
//!    the same stored instant read through two paths is one value.
//!
//! The rows differ along what those rules generalise over: the resource kind, the read
//! path, the lifecycle status the record is read in, the document the record holds, the
//! tag count, the namespace name and the signing region. The request harness (`Amp`,
//! `Reply`) and the create/describe/list helpers are the ones the sibling suites build.

use crate::alert_manager_definition::{
    block_scalar, create as create_definition, delete as delete_definition,
    describe as describe_definition, flow_style, non_ascii, put as put_definition,
    with_template_files,
};
use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace, describe_namespace};
use crate::rule_groups_namespace_listing::list_namespaces;
use crate::rule_groups_namespace_update::{put_body, put_namespace};
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    wire_instant, DEFINITION_STATUS_ACTIVE, DEFINITION_STATUS_CREATING,
    DEFINITION_STATUS_DELETING, DEFINITION_STATUS_UPDATING, NAMESPACE_STATUS_ACTIVE,
    NAMESPACE_STATUS_CREATING, NAMESPACE_STATUS_DELETING, NAMESPACE_STATUS_UPDATING,
    NO_FAILURE_STATUS_REASON,
};
use std::time::{Duration, UNIX_EPOCH};

/// How far a published epoch-seconds number may sit from a whole number of milliseconds
/// and still count as one: an `f64` holds today's epoch seconds to about a quarter of a
/// microsecond, i.e. to about 2.4e-4 of a millisecond, so this slack admits that
/// representation error and nothing coarser. The defect this pins — the clock's full
/// remainder — is off by tenths of a millisecond or more.
const MILLISECOND_SLACK: f64 = 0.01;

/// A published timestamp as the epoch-seconds number the wire carries.
fn instant(case: &str, member: &str, value: &serde_json::Value) -> f64 {
    value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an epoch-seconds number, got {value:?}")
    })
}

/// Assert one published timestamp carries at most millisecond resolution: it is a whole
/// number of milliseconds, and the number as it is RENDERED carries no more than three
/// fractional digits (the shape of every epoch-seconds instant this service publishes).
fn assert_millisecond_instant(case: &str, member: &str, value: &serde_json::Value) -> f64 {
    let seconds = instant(case, member, value);
    let millis = seconds * 1_000.0;
    assert!(
        (millis - millis.round()).abs() < MILLISECOND_SLACK,
        "case {case:?}: {member} must be a whole number of milliseconds, got {seconds} \
         ({millis} ms)"
    );
    let rendered = value.to_string();
    if !rendered.contains('e') && !rendered.contains('E') {
        let digits = rendered
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len());
        assert!(
            digits <= 3,
            "case {case:?}: {member} must carry at most three fractional digits, got \
             {rendered} ({digits} digits)"
        );
    }
    seconds
}

/// Assert a status structure reports `statusCode` and a `statusReason` that is PRESENT and
/// equal to `reason` — never an absent member.
fn assert_status(case: &str, status: &serde_json::Value, status_code: &str, reason: &str) {
    assert_eq!(
        text(&status["statusCode"]),
        status_code,
        "case {case:?}: expected statusCode {status_code}, got {status:?}"
    );
    assert_eq!(
        status["statusReason"].as_str(),
        Some(reason),
        "case {case:?}: statusReason must be present and {reason:?}, got {:?}",
        status["statusReason"]
    );
}

/// Which lifecycle status a row reads its record in: every status a rule groups namespace
/// or an alert manager definition reaches here, since the rule holds in all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadIn {
    /// Read straight after the create, before the clock owes the move to `ACTIVE`.
    Creating,
    /// Read once the clock has settled the record.
    Active,
    /// Read while an accepted update is pending.
    Updating,
    /// Read while an accepted deletion is pending — the record is still stored.
    Deleting,
}

/// One row of the rule groups namespace table.
struct NamespaceCase {
    case: &'static str,
    region: &'static str,
    /// The namespace name, spanning the modelled `RuleGroupsNamespaceName` character class
    /// `[0-9A-Za-z][-.0-9A-Z_a-z]*` and its length bound of 1..128.
    name: String,
    /// The rules file the namespace holds: a blob, so any bytes at all.
    data: Vec<u8>,
    tags: Vec<(String, String)>,
    read_in: ReadIn,
    status_code: &'static str,
}

/// The table spans the inputs the rules generalise over: the lifecycle status the record
/// is read in, the rules file's bytes (text, empty, every byte value, kilobytes), the tag
/// count, the name's length and character class, and the signing region.
fn namespace_cases() -> Vec<NamespaceCase> {
    let rules_file =
        b"groups:\n  - name: cpu\n    rules:\n      - record: job:cpu\n        expr: sum(rate(cpu[5m]))\n"
            .to_vec();
    vec![
        NamespaceCase {
            case: "CREATING, a prometheus rules file, one tag",
            region: "us-west-2",
            name: "rulesfile1".to_string(),
            data: rules_file.clone(),
            tags: vec![("team".to_string(), "observability".to_string())],
            read_in: ReadIn::Creating,
            status_code: NAMESPACE_STATUS_CREATING,
        },
        NamespaceCase {
            case: "ACTIVE, an empty rules file, no tags",
            region: "us-west-2",
            name: "e".to_string(),
            data: Vec::new(),
            tags: Vec::new(),
            read_in: ReadIn::Active,
            status_code: NAMESPACE_STATUS_ACTIVE,
        },
        NamespaceCase {
            case: "ACTIVE, every byte value, three tags",
            region: "us-west-2",
            name: "all-bytes.v2_0".to_string(),
            data: (0u8..=255).collect(),
            tags: vec![
                ("a".to_string(), String::new()),
                ("b".to_string(), "two".to_string()),
                ("c".to_string(), "三".to_string()),
            ],
            read_in: ReadIn::Active,
            status_code: NAMESPACE_STATUS_ACTIVE,
        },
        NamespaceCase {
            case: "UPDATING, kilobytes of rules, name at the 128-byte bound",
            region: "eu-central-1",
            name: "n".repeat(128),
            data: rules_file.repeat(64),
            tags: Vec::new(),
            read_in: ReadIn::Updating,
            status_code: NAMESPACE_STATUS_UPDATING,
        },
        NamespaceCase {
            case: "DELETING, a one-byte rules file, another region",
            region: "ap-southeast-2",
            name: "0".to_string(),
            data: vec![b'#'],
            tags: vec![("owner".to_string(), "sre".to_string())],
            read_in: ReadIn::Deleting,
            status_code: NAMESPACE_STATUS_DELETING,
        },
    ]
}

/// The summary one listing carries for `name`.
fn summary_for(case: &str, listed: &Reply, name: &str) -> serde_json::Value {
    let entries = listed.body["ruleGroupsNamespaces"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ruleGroupsNamespaces must be a list, got {listed:?}")
        });
    entries
        .iter()
        .find(|entry| text(&entry["name"]) == name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the listing must carry {name:?}, got {listed:?}"))
}

/// Rule 1 and rule 2 on the rule groups namespace read paths: the describe and the
/// listing both publish the no-failure `statusReason`, both publish whole-millisecond
/// instants, and the summary reports the SAME values as the description of the same
/// record.
#[tokio::test]
async fn namespace_read_paths_publish_the_status_reason_and_millisecond_instants() {
    let amp = Amp::new();
    for case in namespace_cases() {
        let label = case.case;
        let workspace_id = amp.active_workspace(case.region, label).await;
        let created = create_namespace(
            &amp,
            case.region,
            &workspace_id,
            &create_body(&case.name, &case.data, &case.tags),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {label:?}: the create must be accepted, got {created:?}"
        );

        match case.read_in {
            ReadIn::Creating => {}
            ReadIn::Active => wait_for_due_transition(),
            ReadIn::Updating => {
                wait_for_due_transition();
                let put = put_namespace(
                    &amp,
                    case.region,
                    &workspace_id,
                    &case.name,
                    &put_body(&case.data),
                )
                .await;
                assert_eq!(
                    put.status, 202,
                    "case {label:?}: the update must be accepted, got {put:?}"
                );
            }
            ReadIn::Deleting => {
                wait_for_due_transition();
                let deleted = amp
                    .call(
                        "DELETE",
                        &format!(
                            "/workspaces/{workspace_id}/rulegroupsnamespaces/{}",
                            case.name
                        ),
                        case.region,
                        "",
                    )
                    .await;
                assert_eq!(
                    deleted.status, 202,
                    "case {label:?}: the delete must be accepted, got {deleted:?}"
                );
            }
        }

        let described = describe_namespace(&amp, case.region, &workspace_id, &case.name).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: the stored namespace must be describable, got {described:?}"
        );
        let record = &described.body["ruleGroupsNamespace"];
        assert_status(
            label,
            &record["status"],
            case.status_code,
            NO_FAILURE_STATUS_REASON,
        );
        let created_at = assert_millisecond_instant(label, "createdAt", &record["createdAt"]);
        let modified_at = assert_millisecond_instant(label, "modifiedAt", &record["modifiedAt"]);
        assert!(
            modified_at >= created_at,
            "case {label:?}: modifiedAt {modified_at} must not precede createdAt {created_at}"
        );

        // A read publishes the stored instants, so reading again reports the same numbers.
        let again = describe_namespace(&amp, case.region, &workspace_id, &case.name).await;
        assert_eq!(
            (
                again.body["ruleGroupsNamespace"]["createdAt"].as_f64(),
                again.body["ruleGroupsNamespace"]["modifiedAt"].as_f64()
            ),
            (Some(created_at), Some(modified_at)),
            "case {label:?}: the instants must be stable across reads, got {again:?}"
        );

        let listed = list_namespaces(&amp, case.region, &workspace_id, &[]).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: the listing must answer 200, got {listed:?}"
        );
        let summary = summary_for(label, &listed, &case.name);
        assert_status(
            label,
            &summary["status"],
            case.status_code,
            NO_FAILURE_STATUS_REASON,
        );
        assert_millisecond_instant(label, "the summary's createdAt", &summary["createdAt"]);
        assert_millisecond_instant(label, "the summary's modifiedAt", &summary["modifiedAt"]);
        assert_eq!(
            (summary["createdAt"].as_f64(), summary["modifiedAt"].as_f64()),
            (Some(created_at), Some(modified_at)),
            "case {label:?}: the summary must report the same instants as the \
             description, got {summary:?}"
        );
        assert_eq!(
            summary["status"], record["status"],
            "case {label:?}: the summary must report the same status as the description, \
             got {summary:?} against {record:?}"
        );
    }
}

/// The reasons a record may hold, spanning what a free-form `String` member carries: the
/// empty reason, a sentence, a non-ASCII one, and a long one.
const SEEDED_REASONS: &[&str] = &[
    NO_FAILURE_STATUS_REASON,
    "The rules file could not be loaded.",
    "Échec de la validation des règles",
    "a reason long enough to span several reads of the record and still be reported whole",
];

/// Rule 1's other half: a reason the RECORD holds is published as it stands, by the
/// describe and by the listing alike — so the member reports stored state rather than one
/// constant.
#[tokio::test]
async fn a_stored_namespace_status_reason_is_published_as_it_stands() {
    let region = "us-east-1";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "seeded-reasons").await;
    let name = "rulesfile1";
    let created = create_namespace(
        &amp,
        region,
        &workspace_id,
        &create_body(name, b"groups: []\n", &[]),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the create must be accepted, got {created:?}"
    );
    wait_for_due_transition();

    for reason in SEEDED_REASONS {
        {
            let store = amp.store();
            let mut guard = store.lock();
            let namespace = guard
                .region_mut_for_create(region)
                .workspaces
                .get_mut(&workspace_id)
                .expect("the workspace this suite created")
                .rule_groups_namespaces
                .get_mut(name)
                .expect("the namespace this suite created");
            namespace.status_reason = Some((*reason).to_string());
        }

        let described = describe_namespace(&amp, region, &workspace_id, name).await;
        assert_status(reason, &described.body["ruleGroupsNamespace"]["status"], NAMESPACE_STATUS_ACTIVE, reason);
        let listed = list_namespaces(&amp, region, &workspace_id, &[]).await;
        let summary = summary_for(reason, &listed, name);
        assert_status(reason, &summary["status"], NAMESPACE_STATUS_ACTIVE, reason);
    }
}

/// One row of the alert manager definition table.
struct DefinitionCase {
    case: &'static str,
    region: &'static str,
    /// The definition document, in the forms the `AlertManagerDefinitionData` reference
    /// allows.
    data: Vec<u8>,
    read_in: ReadIn,
    status_code: &'static str,
}

/// The table spans the lifecycle status the definition is read in, the document's YAML
/// form and character set, and the signing region.
fn definition_cases() -> Vec<DefinitionCase> {
    vec![
        DefinitionCase {
            case: "CREATING, a block scalar configuration",
            region: "us-west-2",
            data: block_scalar(),
            read_in: ReadIn::Creating,
            status_code: DEFINITION_STATUS_CREATING,
        },
        DefinitionCase {
            case: "ACTIVE, a configuration with template files",
            region: "us-west-2",
            data: with_template_files(),
            read_in: ReadIn::Active,
            status_code: DEFINITION_STATUS_ACTIVE,
        },
        DefinitionCase {
            case: "UPDATING, a flow-style document",
            region: "eu-west-1",
            data: flow_style(),
            read_in: ReadIn::Updating,
            status_code: DEFINITION_STATUS_UPDATING,
        },
        DefinitionCase {
            case: "DELETING, a non-ASCII document",
            region: "ap-northeast-1",
            data: non_ascii(),
            read_in: ReadIn::Deleting,
            status_code: DEFINITION_STATUS_DELETING,
        },
    ]
}

/// Rule 1 and rule 2 on `DescribeAlertManagerDefinition`: the read publishes the
/// no-failure `statusReason` and whole-millisecond instants in every lifecycle status, and
/// `createdAt` stands across reads.
#[tokio::test]
async fn the_alert_manager_definition_read_publishes_the_status_reason_and_millisecond_instants() {
    let amp = Amp::new();
    for case in definition_cases() {
        let label = case.case;
        let workspace_id = amp.active_workspace(case.region, label).await;
        let created =
            create_definition(&amp, case.region, &workspace_id, &case.data, None).await;
        assert_eq!(
            created.status, 202,
            "case {label:?}: the create must be accepted, got {created:?}"
        );

        match case.read_in {
            ReadIn::Creating => {}
            ReadIn::Active => wait_for_due_transition(),
            ReadIn::Updating => {
                wait_for_due_transition();
                let put = put_definition(&amp, case.region, &workspace_id, &case.data, None).await;
                assert_eq!(
                    put.status, 202,
                    "case {label:?}: the update must be accepted, got {put:?}"
                );
            }
            ReadIn::Deleting => {
                wait_for_due_transition();
                let deleted = delete_definition(&amp, case.region, &workspace_id, None).await;
                assert_eq!(
                    deleted.status, 202,
                    "case {label:?}: the delete must be accepted, got {deleted:?}"
                );
            }
        }

        let described = describe_definition(&amp, case.region, &workspace_id).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: the stored definition must be describable, got {described:?}"
        );
        let record = &described.body["alertManagerDefinition"];
        assert_status(
            label,
            &record["status"],
            case.status_code,
            NO_FAILURE_STATUS_REASON,
        );
        let created_at = assert_millisecond_instant(label, "createdAt", &record["createdAt"]);
        let modified_at = assert_millisecond_instant(label, "modifiedAt", &record["modifiedAt"]);
        assert!(
            modified_at >= created_at,
            "case {label:?}: modifiedAt {modified_at} must not precede createdAt {created_at}"
        );

        let again = describe_definition(&amp, case.region, &workspace_id).await;
        assert_eq!(
            (
                again.body["alertManagerDefinition"]["createdAt"].as_f64(),
                again.body["alertManagerDefinition"]["modifiedAt"].as_f64()
            ),
            (Some(created_at), Some(modified_at)),
            "case {label:?}: the instants must be stable across reads, got {again:?}"
        );
    }
}

/// Rule 1's other half for the definition: a reason the RECORD holds is what the read
/// publishes.
#[tokio::test]
async fn a_stored_definition_status_reason_is_published_as_it_stands() {
    let region = "eu-north-1";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "seeded-definition").await;
    let created = create_definition(&amp, region, &workspace_id, &block_scalar(), None).await;
    assert_eq!(
        created.status, 202,
        "the create must be accepted, got {created:?}"
    );
    wait_for_due_transition();

    for reason in SEEDED_REASONS {
        {
            let store = amp.store();
            let mut guard = store.lock();
            let definition = guard
                .region_mut_for_create(region)
                .workspaces
                .get_mut(&workspace_id)
                .expect("the workspace this suite created")
                .alert_manager_definition
                .as_mut()
                .expect("the definition this suite created");
            definition.status_reason = Some((*reason).to_string());
        }

        let described = describe_definition(&amp, region, &workspace_id).await;
        assert_status(
            reason,
            &described.body["alertManagerDefinition"]["status"],
            DEFINITION_STATUS_ACTIVE,
            reason,
        );
    }
}

/// Rule 2 on the projection itself, over instants no HTTP row reaches: every published
/// instant is its stored instant TRUNCATED to whole milliseconds, which is an idempotent
/// map — truncating an already-truncated instant changes nothing.
///
/// The rows span the epoch, each sub-millisecond remainder a clock can carry, a
/// millisecond boundary, and a far-future instant.
#[test]
fn every_published_instant_is_truncated_to_whole_milliseconds() {
    let rows: &[(&str, Duration)] = &[
        ("the epoch itself", Duration::ZERO),
        ("one nanosecond past a second", Duration::new(1, 1)),
        ("one microsecond past a second", Duration::new(1, 1_000)),
        (
            "999_999 nanoseconds past a millisecond",
            Duration::new(1, 1_999_999),
        ),
        ("an exact millisecond", Duration::new(1, 2_000_000)),
        (
            "a clock remainder with seven fractional digits",
            Duration::new(1_794_228_650, 982_514_900),
        ),
        (
            "a far-future instant",
            Duration::new(4_102_444_800, 123_456_789),
        ),
    ];

    for (case, since_epoch) in rows {
        let stored = UNIX_EPOCH + *since_epoch;
        let published = wire_instant(stored);
        let published_millis = published
            .to_millis()
            .expect("a representable published instant");
        let expected_millis =
            i64::try_from(since_epoch.as_millis()).expect("a representable instant");
        assert_eq!(
            published_millis, expected_millis,
            "case {case:?}: the published instant must be the stored one truncated to \
             milliseconds, got {published:?}"
        );
        assert_eq!(
            published.subsec_nanos() % 1_000_000,
            0,
            "case {case:?}: a published instant carries no sub-millisecond remainder, got \
             {published:?}"
        );
        let republished = wire_instant(
            UNIX_EPOCH + Duration::from_millis(published_millis.unsigned_abs()),
        )
        .to_millis()
        .expect("a representable published instant");
        assert_eq!(
            republished, published_millis,
            "case {case:?}: truncating an already-truncated instant must change nothing, \
             got {published:?}"
        );
    }
}
