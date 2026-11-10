// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeImportSnapshotTasks`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeImportSnapshotTasks`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the import snapshot tasks the calling account holds, narrowed by the `ImportTaskIds`
//! and `Filters` the request names, cut into `MaxResults`-sized pages, with every member a
//! writer stored reported back unchanged — and that describing changes nothing a later
//! caller sees.
//!
//! Each table row is a distinct input along what that rule generalises over — which task
//! state, which filter, which values, how many ids, which page size, a wildcard against a
//! literal — so no single row is the specification. The store is seeded through
//! [`ec2::state::ComAmazonawsEc2Amazonec2State::insert_import_snapshot_task`], the writer
//! `ImportSnapshot` will use once it is implemented (its own stub issue,
//! `20261022T235108Z-ec2-importsnapshot-stub.md`); the shared harness hands the engine
//! back for exactly this ("so a suite can seed the store a handler reads").

use std::collections::BTreeMap;

use ec2::ops::types::{SnapshotTaskDetail, Tag, UserBucketDetails};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, IMPORT_TASK_STATES, IMPORT_TASK_STATE_ACTIVE,
    IMPORT_TASK_STATE_COMPLETED, IMPORT_TASK_STATE_DELETED, IMPORT_TASK_STATE_DELETING,
    ImportSnapshotTaskSpec, MAX_PAGE_SIZE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{elements, error_code, indexed};

/// The Region every call here is signed for. An `ImportSnapshotTask` carries no Region
/// member, so the store is per account and the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// One `(key, value)` request parameter.
fn param(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a successful report: `(case, the parameters under
/// test, the task ids it must select, in the order they must be reported)`.
type SelectionRow = (&'static str, Params, Vec<String>);

/// One row of a table that expects a refusal: `(case, the parameters under test, the EC2
/// error code the refusal must carry)`.
type RefusalRow = (&'static str, Params, &'static str);

/// One `DescribeImportSnapshotTasks` call carrying `params`, as its HTTP status and XML
/// body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeImportSnapshotTasks", params).await
}

/// The form fields for a whole `Filters` list, as ec2Query flattens it for THIS operation.
///
/// `DescribeImportSnapshotTasksRequest$Filters` carries `@xmlName("Filters")`
/// (`smithy_query(query="operation",operation="DescribeImportSnapshotTasks")`), so its
/// query key is `Filters.N` — not the `Filter.N` of the sibling describes, which is why
/// this is not the shared harness's `filters`.
fn filter_params(entries: &[(&str, &[&str])]) -> Vec<(String, String)> {
    entries
        .iter()
        .enumerate()
        .flat_map(|(index, (name, values))| {
            let position = index + 1;
            let mut fields = vec![param(&format!("Filters.{position}.Name"), *name)];
            fields.extend(indexed(&format!("Filters.{position}.Value"), values));
            fields
        })
        .collect()
}

/// `ImportTaskId.N` fields, the key `DescribeImportSnapshotTasksRequest$ImportTaskIds`
/// carries (`@xmlName("ImportTaskId")`).
fn id_params(ids: &[&str]) -> Vec<(String, String)> {
    indexed("ImportTaskId", ids)
}

/// A `MaxResults` field.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![param("MaxResults", max_results.to_string())]
}

fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// The tasks the store is seeded with, by case name: one per task state the import-task
/// documentation names, plus a task whose detail reports no status at all, and they differ
/// along every member the assertions below read — the disk image source (an S3 object
/// against a URL), the format, the size, encryption, the tags, and whether a snapshot
/// exists yet.
fn seeded_rows() -> Vec<(&'static str, ImportSnapshotTaskSpec)> {
    vec![
        (
            "active from an S3 object",
            ImportSnapshotTaskSpec {
                description: Some("nightly appliance disk".to_string()),
                detail: SnapshotTaskDetail {
                    status: Some(IMPORT_TASK_STATE_ACTIVE.to_string()),
                    status_message: Some("pending".to_string()),
                    progress: Some("43".to_string()),
                    format: Some("VMDK".to_string()),
                    disk_image_size: Some(8.0),
                    encrypted: Some(false),
                    user_bucket: Some(UserBucketDetails {
                        s3_bucket: Some("disk-images".to_string()),
                        s3_key: Some("appliance.vmdk".to_string()),
                    }),
                    ..SnapshotTaskDetail::default()
                },
                tags: vec![tag("Project", "vmimport"), tag("Owner", "TeamA")],
            },
        ),
        (
            "completed from a URL, encrypted",
            ImportSnapshotTaskSpec {
                description: Some("golden volume".to_string()),
                detail: SnapshotTaskDetail {
                    status: Some(IMPORT_TASK_STATE_COMPLETED.to_string()),
                    progress: Some("100".to_string()),
                    format: Some("RAW".to_string()),
                    disk_image_size: Some(512.5),
                    encrypted: Some(true),
                    kms_key_id: Some(
                        "arn:aws:kms:us-east-1:012345678910:key/abcd1234-a123-456a-a12b-a123b4cd56ef"
                            .to_string(),
                    ),
                    snapshot_id: Some("snap-0123456789abcdef0".to_string()),
                    url: Some("https://example.com/disks/golden.raw".to_string()),
                    ..SnapshotTaskDetail::default()
                },
                tags: vec![tag("Owner", "TeamB")],
            },
        ),
        (
            "deleting, untagged",
            ImportSnapshotTaskSpec {
                description: Some("cancelled mid-import".to_string()),
                detail: SnapshotTaskDetail {
                    status: Some(IMPORT_TASK_STATE_DELETING.to_string()),
                    progress: Some("12".to_string()),
                    format: Some("VHD".to_string()),
                    ..SnapshotTaskDetail::default()
                },
                tags: Vec::new(),
            },
        ),
        (
            "deleted with a reason",
            ImportSnapshotTaskSpec {
                description: Some("failed disk validation".to_string()),
                detail: SnapshotTaskDetail {
                    status: Some(IMPORT_TASK_STATE_DELETED.to_string()),
                    status_message: Some("ClientError: disk validation failed".to_string()),
                    progress: Some("0".to_string()),
                    format: Some("VMDK".to_string()),
                    ..SnapshotTaskDetail::default()
                },
                tags: vec![tag("Project", "retired")],
            },
        ),
        (
            "no status recorded",
            ImportSnapshotTaskSpec {
                description: Some("a task whose detail carries no status".to_string()),
                detail: SnapshotTaskDetail::default(),
                tags: Vec::new(),
            },
        ),
    ]
}

/// What a seeded store holds: the id of each seeded task by case name, and those ids in
/// the order the describe reports them.
struct Seeded {
    ids_by_case: BTreeMap<&'static str, String>,
    ids: Vec<String>,
}

impl Seeded {
    /// The id of one seeded task by case name.
    fn id(&self, case: &str) -> String {
        self.ids_by_case
            .get(case)
            .unwrap_or_else(|| panic!("{case} must be a seeded import snapshot task"))
            .clone()
    }

    /// The ids of `cases`, in the order the describe reports them — ids are the zero-padded
    /// hex of a monotonic counter, so sorting them is the store's own order.
    fn ids_of(&self, cases: &[&str]) -> Vec<String> {
        let mut ids: Vec<String> = cases.iter().map(|case| self.id(case)).collect();
        ids.sort();
        ids
    }
}

/// Write the seeded tasks into the engine's store, the way `ImportSnapshot` will, and
/// report the ids they were given.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids_by_case = BTreeMap::new();
    for (case, spec) in seeded_rows() {
        assert!(
            guard.has_import_snapshot_task_capacity(),
            "case {case:?}: the store must have room for another import snapshot task"
        );
        let import_task_id = guard
            .insert_import_snapshot_task(spec)
            .unwrap_or_else(|| panic!("case {case:?}: the store must store the task"));
        ids_by_case.insert(case, import_task_id);
    }
    let ids: Vec<String> = guard.import_snapshot_tasks.keys().cloned().collect();
    Seeded { ids_by_case, ids }
}

/// The task ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "importTaskId")
}

/// The text of `node`'s `name` child, or `None` when it has none.
fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    node.children()
        .find(|child| child.has_tag_name(name))
        .and_then(|child| child.text())
        .map(str::to_string)
}

/// One reported task, as the assertions read it: the members of the task and of its
/// `SnapshotTaskDetail`, plus its tags as `key=value` pairs.
#[derive(Debug, Clone, PartialEq)]
struct TaskRow {
    import_task_id: String,
    description: Option<String>,
    detail_description: Option<String>,
    status: Option<String>,
    status_message: Option<String>,
    progress: Option<String>,
    format: Option<String>,
    disk_image_size: Option<f64>,
    encrypted: Option<bool>,
    kms_key_id: Option<String>,
    snapshot_id: Option<String>,
    url: Option<String>,
    s3_bucket: Option<String>,
    s3_key: Option<String>,
    tags: Vec<String>,
}

/// Every task of a response body, as rows, keyed by the task id.
fn task_rows(body: &str) -> BTreeMap<String, TaskRow> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    document
        .descendants()
        .filter(|node| {
            node.is_element() && node.children().any(|child| child.has_tag_name("importTaskId"))
        })
        .map(|node| {
            let detail = node
                .children()
                .find(|child| child.has_tag_name("snapshotTaskDetail"));
            let bucket = detail.and_then(|detail| {
                detail
                    .children()
                    .find(|child| child.has_tag_name("userBucket"))
            });
            let import_task_id = child_text(node, "importTaskId").unwrap_or_default();
            let tags = node
                .children()
                .find(|child| child.has_tag_name("tagSet"))
                .map(|tag_set| {
                    tag_set
                        .children()
                        .filter(|item| item.is_element())
                        .map(|item| {
                            format!(
                                "{}={}",
                                child_text(item, "key").unwrap_or_default(),
                                child_text(item, "value").unwrap_or_default()
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let row = TaskRow {
                import_task_id: import_task_id.clone(),
                description: child_text(node, "description"),
                detail_description: detail.and_then(|detail| child_text(detail, "description")),
                status: detail.and_then(|detail| child_text(detail, "status")),
                status_message: detail.and_then(|detail| child_text(detail, "statusMessage")),
                progress: detail.and_then(|detail| child_text(detail, "progress")),
                format: detail.and_then(|detail| child_text(detail, "format")),
                disk_image_size: detail
                    .and_then(|detail| child_text(detail, "diskImageSize"))
                    .and_then(|size| size.parse::<f64>().ok()),
                encrypted: detail
                    .and_then(|detail| child_text(detail, "encrypted"))
                    .and_then(|encrypted| encrypted.parse::<bool>().ok()),
                kms_key_id: detail.and_then(|detail| child_text(detail, "kmsKeyId")),
                snapshot_id: detail.and_then(|detail| child_text(detail, "snapshotId")),
                url: detail.and_then(|detail| child_text(detail, "url")),
                s3_bucket: bucket.and_then(|bucket| child_text(bucket, "s3Bucket")),
                s3_key: bucket.and_then(|bucket| child_text(bucket, "s3Key")),
                tags,
            };
            (import_task_id, row)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what gets reported
// ---------------------------------------------------------------------------

/// A fresh account holds no import snapshot tasks, so the describe answers an empty set —
/// "a list of zero or more import snapshot tasks"
/// (`DescribeImportSnapshotTasksResult$ImportSnapshotTasks`) — with no page token, and
/// asking twice answers the same both times.
#[tokio::test]
async fn a_fresh_account_reports_no_import_snapshot_tasks() {
    let (router, _engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "importTaskId").is_empty(),
        "fresh account: expected no tasks, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "fresh account: expected no page token, got {body}"
    );

    let (_, again) = describe(&router, &[]).await;
    assert_eq!(
        task_rows(&body),
        task_rows(&again),
        "fresh account: describing must not change what a later caller sees"
    );
}

/// Every member a writer stored round-trips through the describe, on the element the model
/// names for it, and the members the service owns (the task id) are derived rather than
/// taken. One row per seeded task, so the assertion spans both disk-image sources, all
/// four task states, an encrypted and an unencrypted import, a task with a snapshot and
/// tasks without, and a task with no status at all.
#[tokio::test]
async fn every_stored_member_round_trips_through_the_describe() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "seeded store: expected HTTP 200, got {status} with body {body}"
    );
    let reported = task_rows(&body);

    for (case, spec) in seeded_rows() {
        let id = seeded.id(case);
        let row = reported
            .get(&id)
            .unwrap_or_else(|| panic!("case {case:?}: expected {id} in {body}"));
        let expected = TaskRow {
            import_task_id: id.clone(),
            description: spec.description.clone(),
            // One request field, reported on the task and on its detail alike.
            detail_description: spec.description.clone(),
            status: spec.detail.status.clone(),
            status_message: spec.detail.status_message.clone(),
            progress: spec.detail.progress.clone(),
            format: spec.detail.format.clone(),
            disk_image_size: spec.detail.disk_image_size,
            encrypted: spec.detail.encrypted,
            kms_key_id: spec.detail.kms_key_id.clone(),
            snapshot_id: spec.detail.snapshot_id.clone(),
            url: spec.detail.url.clone(),
            s3_bucket: spec
                .detail
                .user_bucket
                .as_ref()
                .and_then(|bucket| bucket.s3_bucket.clone()),
            s3_key: spec
                .detail
                .user_bucket
                .as_ref()
                .and_then(|bucket| bucket.s3_key.clone()),
            tags: spec
                .tags
                .iter()
                .map(|tag| {
                    format!(
                        "{}={}",
                        tag.key.clone().unwrap_or_default(),
                        tag.value.clone().unwrap_or_default()
                    )
                })
                .collect(),
        };
        assert_eq!(
            row, &expected,
            "case {case:?}: expected the stored task reported back unchanged, got {row:?}"
        );
        assert!(
            id.starts_with("import-snap-") && id.len() > "import-snap-".len(),
            "case {case:?}: expected a service-minted import task id, got {id:?}"
        );
    }

    assert_eq!(
        reported.len(),
        seeded.ids.len(),
        "seeded store: expected every stored task reported, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// Narrowing by id
// ---------------------------------------------------------------------------

/// `ImportTaskIds` narrows the report to the tasks it names, in store order, however many
/// it names and however often it repeats one.
#[tokio::test]
async fn import_task_ids_narrow_the_report_to_the_ids_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let one = seeded.id("deleting, untagged");
    let other = seeded.id("completed from a URL, encrypted");
    let rows: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        ("no ids names every task", vec![], seeded.ids.clone()),
        (
            "one id",
            vec![one.as_str()],
            vec![one.clone()],
        ),
        (
            "two ids, reported in store order",
            vec![other.as_str(), one.as_str()],
            seeded.ids_of(&["deleting, untagged", "completed from a URL, encrypted"]),
        ),
        (
            "the same id twice names it once",
            vec![one.as_str(), one.as_str()],
            vec![one.clone()],
        ),
    ];

    for (case, ids, expected) in rows {
        let reported = described_ids(&router, case, &id_params(&ids)).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// An `ImportTaskIds` entry that is not an import snapshot task id at all is refused with
/// EC2's own conversion-task-id code, there being nothing to look up; a WELL-FORMED id the
/// account holds no task for simply selects no row, which is the rule the sibling
/// `DescribeImportImageTasks` holds for the same member.
#[tokio::test]
async fn a_malformed_import_task_id_is_refused_and_an_unknown_one_selects_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let known = seeded.id("active from an S3 object");
    let malformed: Vec<(&str, Vec<&str>)> = vec![
        ("an id of another resource kind", vec!["snap-0123456789abcdef0"]),
        ("not an id at all", vec!["not-an-id"]),
        ("the prefix with no digits", vec!["import-snap-"]),
        ("the right prefix, a non-hex digit", vec!["import-snap-00000000000000ffg"]),
        (
            "a malformed id beside a known one",
            vec![known.as_str(), "not-an-id"],
        ),
    ];

    for (case, ids) in malformed {
        let (status, body) = describe(&router, &id_params(&ids)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidConversionTaskId",
            "case {case:?}: expected EC2's conversion-task-id code, got {body}"
        );
    }

    // A well-formed id of a task the account does not hold is not an error: it selects no
    // row, alone or beside an id that does name a task. The suffix's LENGTH is no part of
    // what the member admits, so a hexadecimal suffix EC2 mints no id in reads as an id
    // just the same.
    let unknown: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        (
            "a short-form id that names nothing",
            vec!["import-snap-00000fff"],
            Vec::new(),
        ),
        (
            "a long-form id that names nothing",
            vec!["import-snap-00000000000000fff"],
            Vec::new(),
        ),
        (
            "an id in no minted length that names nothing",
            vec!["import-snap-0ff"],
            Vec::new(),
        ),
        (
            "an unknown id beside a known one",
            vec![known.as_str(), "import-snap-00000000000000fff"],
            vec![known.clone()],
        ),
    ];
    for (case, ids, expected) in unknown {
        let reported = described_ids(&router, case, &id_params(&ids)).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // Nothing was stored or removed by the refusals.
    let after = described_ids(&router, "after the refusals", &[]).await;
    assert_eq!(
        after, seeded.ids,
        "after the refusals: expected the store unchanged, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each filter this operation answers selects the tasks whose attribute matches one of the
/// values, case-sensitively and with EC2's `*`/`?` wildcards; several filters are joined
/// with an AND, and a filter carrying no value selects nothing.
#[tokio::test]
async fn filters_select_the_tasks_whose_attribute_matches_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The `task-state` rows cover every value the import-task documentation names
    // (`ec2::state::IMPORT_TASK_STATES`), read off that catalog rather than copied, so a
    // state nobody cited is covered too.
    let state_cases = [
        (IMPORT_TASK_STATE_ACTIVE, "active from an S3 object"),
        (
            IMPORT_TASK_STATE_COMPLETED,
            "completed from a URL, encrypted",
        ),
        (IMPORT_TASK_STATE_DELETING, "deleting, untagged"),
        (IMPORT_TASK_STATE_DELETED, "deleted with a reason"),
    ];
    for state in IMPORT_TASK_STATES {
        let case = format!("task-state={state}");
        let expected_case = state_cases
            .iter()
            .find(|(value, _)| *value == state)
            .map(|(_, seeded_case)| *seeded_case)
            .unwrap_or_else(|| panic!("case {case:?}: every documented state must have a row"));
        let reported =
            described_ids(&router, &case, &filter_params(&[("task-state", &[state])])).await;
        assert_eq!(
            reported,
            seeded.ids_of(&[expected_case]),
            "case {case:?}: expected only the task in that state, got {reported:?}"
        );
    }

    let rows: Vec<SelectionRow> = vec![
        (
            "two values are joined with an OR",
            filter_params(&[(
                "task-state",
                &[IMPORT_TASK_STATE_ACTIVE, IMPORT_TASK_STATE_DELETED],
            )]),
            seeded.ids_of(&["active from an S3 object", "deleted with a reason"]),
        ),
        (
            "a trailing wildcard matches both deleting and deleted",
            filter_params(&[("task-state", &["delet*"])]),
            seeded.ids_of(&["deleting, untagged", "deleted with a reason"]),
        ),
        (
            "a single-character wildcard",
            filter_params(&[("task-state", &["?ctive"])]),
            seeded.ids_of(&["active from an S3 object"]),
        ),
        (
            "filter values are case-sensitive",
            filter_params(&[("task-state", &["ACTIVE"])]),
            Vec::new(),
        ),
        (
            "a state no seeded task is in selects nothing",
            filter_params(&[("task-state", &["converting"])]),
            Vec::new(),
        ),
        (
            "tag-key selects every task carrying that key",
            filter_params(&[("tag-key", &["Owner"])]),
            seeded.ids_of(&[
                "active from an S3 object",
                "completed from a URL, encrypted",
            ]),
        ),
        (
            "tag:<key> selects on the value under that key",
            filter_params(&[("tag:Owner", &["TeamB"])]),
            seeded.ids_of(&["completed from a URL, encrypted"]),
        ),
        (
            "tag:<key> with a wildcard value",
            filter_params(&[("tag:Owner", &["Team*"])]),
            seeded.ids_of(&[
                "active from an S3 object",
                "completed from a URL, encrypted",
            ]),
        ),
        (
            "two filters are joined with an AND",
            filter_params(&[
                ("task-state", &[IMPORT_TASK_STATE_ACTIVE]),
                ("tag:Owner", &["TeamA"]),
            ]),
            seeded.ids_of(&["active from an S3 object"]),
        ),
        (
            "two filters nothing satisfies together",
            filter_params(&[
                ("task-state", &[IMPORT_TASK_STATE_COMPLETED]),
                ("tag:Owner", &["TeamA"]),
            ]),
            Vec::new(),
        ),
        (
            "a filter with no value matches nothing",
            filter_params(&[("task-state", &[])]),
            Vec::new(),
        ),
        (
            "a task whose detail carries no status matches no state",
            filter_params(&[("task-state", &["*"])]),
            seeded.ids_of(&[
                "active from an S3 object",
                "completed from a URL, encrypted",
                "deleting, untagged",
                "deleted with a reason",
            ]),
        ),
    ];

    for (case, params, expected) in rows {
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not answer is refused rather than read as "no
/// results", whatever it is a near-miss of.
#[tokio::test]
async fn a_filter_name_this_operation_does_not_answer_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let rows: Vec<(&'static str, Params)> = vec![
        ("a name of no EC2 filter", filter_params(&[("nonsense", &["x"])])),
        (
            "the attribute's own member name",
            filter_params(&[("status", &[IMPORT_TASK_STATE_ACTIVE])]),
        ),
        (
            "a filter of the sibling resource",
            filter_params(&[("snapshot-id", &["snap-0123456789abcdef0"])]),
        ),
        (
            "filter names are case-sensitive",
            filter_params(&[("Task-State", &[IMPORT_TASK_STATE_ACTIVE])]),
        ),
        ("a tag prefix with no key", filter_params(&[("tag:", &["TeamA"])])),
        (
            "a supported filter beside an unsupported one",
            filter_params(&[
                ("task-state", &[IMPORT_TASK_STATE_ACTIVE]),
                ("nonsense", &["x"]),
            ]),
        ),
    ];

    for (case, params) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every task exactly once, in store
/// order, no page exceeds the size asked for, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_task_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The smallest page, one that divides the store exactly, one that does not, the size
    // that holds everything, and the largest page this service serves.
    let page_sizes = [
        1,
        i64::try_from(seeded.ids.len()).unwrap_or(1) - 1,
        2,
        i64::try_from(seeded.ids.len()).unwrap_or(1),
        i64::try_from(MAX_PAGE_SIZE).unwrap_or(1),
    ];
    for max_results in page_sizes {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "importTaskId");
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.ids.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(param("NextToken", token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: expected every task exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is a cut of the NARROWED set, not of the whole store: `MaxResults` composes with
/// a filter, and the token resumes inside that set.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let expected = seeded.ids_of(&["deleting, untagged", "deleted with a reason"]);
    let mut params = filter_params(&[("task-state", &["delet*"])]);
    params.extend(max_results_param(1));

    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "first page: got {status} with body {body}");
    let first = elements(&body, "importTaskId");
    assert_eq!(
        first,
        expected[..1].to_vec(),
        "first page: expected the first matching task only, got {first:?}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("first page: expected a NextToken in {body}"));

    let mut next = filter_params(&[("task-state", &["delet*"])]);
    next.extend(max_results_param(1));
    next.push(param("NextToken", token));
    let (status, body) = describe(&router, &next).await;
    assert_eq!(status, 200, "second page: got {status} with body {body}");
    let second = elements(&body, "importTaskId");
    assert_eq!(
        second,
        expected[1..].to_vec(),
        "second page: expected the task after the first, got {second:?}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "second page: the filtered set ends here, so no token is due, got {body}"
    );
}

/// A page of none is refused at the boundary; every other page size the model leaves
/// unbounded above answers a page, the ceiling and anything past it included.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let accepted = [
        1_i64,
        i64::try_from(MAX_PAGE_SIZE).unwrap_or(1),
        i64::try_from(MAX_PAGE_SIZE).unwrap_or(1) + 1,
        i64::from(i32::MAX),
    ];
    for max_results in accepted {
        let case = format!("MaxResults={max_results}");
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
    }

    let refused = [0_i64, -1];
    for max_results in refused {
        let case = format!("MaxResults={max_results}");
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// A `NextToken` this service did not mint is refused rather than read as a position, and
/// it is refused whatever else the request carries.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Not hex at all; hex of `ec2-1:0:x`, this service's marker with a checksum that does
    // not match the cursor it carries; and a bare task id, which is what a caller
    // assembling a token by hand would reach for.
    let own_id = seeded.id("active from an S3 object");
    let tokens = ["not-a-token", "6563322d313a303a78", own_id.as_str()];
    for token in tokens {
        let case = format!("NextToken={token:?}");
        let mut params = vec![param("NextToken", token)];
        params.extend(filter_params(&[("task-state", &[IMPORT_TASK_STATE_ACTIVE])]));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports what the request would have done without doing it: the permitted
/// outcome once every parameter has been validated, and the parameter's own refusal when
/// one of them is wrong.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let would_succeed = vec![
        param("DryRun", "true"),
        param("Filters.1.Name", "task-state"),
        param("Filters.1.Value.1", IMPORT_TASK_STATE_ACTIVE),
    ];
    let (status, body) = describe(&router, &would_succeed).await;
    assert_eq!(
        status, 412,
        "a permitted dry run: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a permitted dry run: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "importTaskId").is_empty(),
        "a permitted dry run: no task may be reported, got {body}"
    );

    // A dry run of a request that would have been refused is refused with THAT reason.
    let rows: Vec<RefusalRow> = vec![
        (
            "a dry run with an unsupported filter",
            filter_params(&[("nonsense", &["x"])]),
            "InvalidParameterValue",
        ),
        (
            "a dry run naming a malformed task id",
            id_params(&["not-an-id"]),
            "InvalidConversionTaskId",
        ),
    ];
    for (case, extra, expected_code) in rows {
        let mut params = vec![param("DryRun", "true")];
        params.extend(extra);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Nothing the dry runs touched changed what a later caller sees.
    let after = described_ids(&router, "after the dry runs", &[]).await;
    assert_eq!(
        after, seeded.ids,
        "after the dry runs: expected the store unchanged, got {after:?}"
    );
}
