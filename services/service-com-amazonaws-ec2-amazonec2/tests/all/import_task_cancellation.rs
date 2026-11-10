// SPDX-License-Identifier: CC0-1.0

//! What `CancelImportTask` cancels and what it leaves alone, driven over the wire the
//! service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rules under test are:
//!
//! 1. AN ID THAT CANNOT BE RESOLVED TO AN IMPORT TASK IS REFUSED BEFORE ANYTHING IS
//!    WRITTEN, under the one code this family answers such an id with: 400
//!    `InvalidConversionTaskId`, whether the value names no kind of import task at all
//!    (judged before the store is reached) or names a kind the account holds no such task
//!    under, where the refusal spells that kind. The kind of an import task id is the one
//!    its prefix names — `import-ami-` for an import image task and `import-snap-` for an
//!    import snapshot task, the two kinds `CancelImportTaskRequest$ImportTaskId` documents
//!    — followed by a hexadecimal suffix of any length;
//! 2. A CANCEL THE TASK'S STATE DOES NOT ADMIT IS REFUSED with `IncorrectState` and writes
//!    nothing: "cancels an IN-PROCESS import virtual machine or import snapshot task"
//!    (`CancelImportTask`), so a task that is not importing any more has nothing left to
//!    cancel;
//! 3. A CANCEL THAT IS ADMITTED MOVES THE STATE OF ONLY THE TASK IT NAMES, into the
//!    non-terminal `deleting` it reports beside the state the task was in, and the clock
//!    owes it `deleted` — so a caller acting straight after the call still sees `deleting`;
//! 4. `DryRun` ANSWERS WHAT THE REQUEST WOULD HAVE DONE and writes nothing: the refusal a
//!    request would have been refused with, or `DryRunOperation` for one that would have
//!    worked.
//!
//! Each row is a distinct input along what the rules generalise over — which kind of import
//! task, which minting prefix, which id length, which way an id departs from that form,
//! which state the task is in, how many disks the import carries, whether a cancel reason
//! was given and how long it is — so no row is the specification. The id rows are GENERATED
//! from the crate's own prefixes, so a form added there is covered here without a new
//! literal.
//!
//! An import image task is started through `ImportImage` itself, so the cancel is asserted
//! against a task the service really minted. An import snapshot task is seeded through
//! [`ec2::state::ComAmazonawsEc2Amazonec2State::insert_import_snapshot_task`], the writer
//! `ImportSnapshot` will use once it is implemented (its own stub issue,
//! `20261022T235108Z-ec2-importsnapshot-stub.md`).

use std::time::{Duration, Instant};

use ec2::ops::types::{ImportImageTask, SnapshotDetail, SnapshotTaskDetail, Tag, UserBucketDetails};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, IMPORT_IMAGE_TASK_ID_PREFIX,
    IMPORT_IMAGE_TASK_ID_PREFIX_WITH_HYPHEN, IMPORT_IMAGE_TASK_TAGGABLE_RESOURCE,
    IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN, IMPORT_SNAPSHOT_TASK_TAGGABLE_RESOURCE,
    IMPORT_TASK_STATE_ACTIVE, IMPORT_TASK_STATE_COMPLETED, IMPORT_TASK_STATE_DELETED,
    IMPORT_TASK_STATE_DELETING, INCORRECT_STATE, INVALID_CONVERSION_TASK_ID,
    INVALID_PARAMETER_VALUE, ImportImageTaskRecord, ImportSnapshotTaskSpec,
    MAX_IMPORT_TASK_CANCEL_REASON_CHARS, MISSING_PARAMETER, TRANSITION_DELAY,
    import_image_task_id_is_well_formed, import_snapshot_task_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, member, unescaped};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for. An import task carries no Region member, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Wait until every clock-owed move started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// Long enough that no clock-owed move of a task seeded with it can fall due while this
/// test runs, so a row about a TRANSIENT state never depends on how long the test took.
const BEYOND_THIS_TEST: Duration = Duration::from_secs(3_600);

/// The suffix lengths an EC2 resource id is minted with: the long (17 hex digit) form EC2
/// mints today and the short (8 hex digit) form it minted before long ids.
const MINTED_SUFFIXES: [(&str, &str); 2] = [
    ("the long (17 hex digit) form", "0123456789abcdef0"),
    ("the short (8 hex digit) form", "0123abcd"),
];

/// Hexadecimal suffix lengths EC2 mints no id in, which
/// `crate::state::import_task_id_is_well_formed` nevertheless reads as this parameter's
/// value: the length of the suffix is no part of what the parameter admits, so such an id
/// names the task of its kind that is not there rather than a value of the wrong format.
const UNMINTED_SUFFIXES: [(&str, &str); 4] = [
    ("a digit short of the short form", "0123abc"),
    ("a digit past the short form", "0123abcde"),
    ("a digit short of the long form", "0123456789abcdef"),
    ("a digit past the long form", "0123456789abcdef01"),
];

/// The two kinds of import task one call can name: "the ID of the import image or import
/// snapshot task to be canceled" (`CancelImportTaskRequest$ImportTaskId`). Every rule here
/// holds for both, so each table row carries the kind it exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// An import IMAGE task, as `ImportImage` starts one.
    Image,
    /// An import SNAPSHOT task, as `ImportSnapshot` will.
    Snapshot,
}

impl Kind {
    /// The describe that reports a task of this kind.
    fn describe_action(self) -> &'static str {
        match self {
            Self::Image => "DescribeImportImageTasks",
            Self::Snapshot => "DescribeImportSnapshotTasks",
        }
    }

    /// The id prefix, with its hyphen, a task of this kind is minted under.
    fn id_prefix(self) -> &'static str {
        match self {
            Self::Image => IMPORT_IMAGE_TASK_ID_PREFIX_WITH_HYPHEN,
            Self::Snapshot => IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN,
        }
    }

    /// The modelled `ResourceType` of this kind of task, which is how a refusal naming such
    /// a task spells its kind.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Image => IMPORT_IMAGE_TASK_TAGGABLE_RESOURCE,
            Self::Snapshot => IMPORT_SNAPSHOT_TASK_TAGGABLE_RESOURCE,
        }
    }
}

/// One `CancelImportTask` call carrying `params`.
async fn cancel(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CancelImportTask", params).await
}

/// One `CancelImportTask` call that names nothing but the task.
async fn cancel_task(router: &MultiServiceRouter, import_task_id: &str) -> (u16, String) {
    cancel(router, &[p("ImportTaskId", import_task_id)]).await
}

/// One import task as its describe reports it: the state the task is in, the status message
/// it carries, and the state of each disk an import image task converts (an import snapshot
/// task's one disk IS its `SnapshotTaskDetail`, so it reports none of its own).
#[derive(Debug, Clone, PartialEq)]
struct ReportedTask {
    state: Option<String>,
    status_message: Option<String>,
    disk_states: Vec<Option<String>>,
}

/// The task `import_task_id`, as the describe of its kind reports it over the wire.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    kind: Kind,
    import_task_id: &str,
) -> ReportedTask {
    let (status, body) = call(
        router,
        REGION,
        kind.describe_action(),
        &[p("ImportTaskId.1", import_task_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {}, got {status} with body {body}",
        kind.describe_action()
    );
    let parsed = document(case, &body);
    let task = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "importTaskId").is_some())
        .unwrap_or_else(|| {
            panic!("case {case:?}: no task for {import_task_id} in {body}")
        });
    match kind {
        Kind::Image => ReportedTask {
            state: child_text(task, "status"),
            status_message: child_text(task, "statusMessage"),
            disk_states: child(task, "snapshotDetailSet")
                .map(|set| {
                    set.children()
                        .filter(roxmltree::Node::is_element)
                        .map(|item| child_text(item, "status"))
                        .collect()
                })
                .unwrap_or_default(),
        },
        Kind::Snapshot => {
            let detail = child(task, "snapshotTaskDetail").unwrap_or_else(|| {
                panic!("case {case:?}: no <snapshotTaskDetail> for {import_task_id} in {body}")
            });
            ReportedTask {
                state: child_text(detail, "status"),
                status_message: child_text(detail, "statusMessage"),
                disk_states: Vec::new(),
            }
        }
    }
}

/// Start one import image task through `ImportImage` itself, over `disks` disk images, and
/// answer the id the service minted for it.
///
/// Its completion is pushed [`BEYOND_THIS_TEST`] straight afterwards: what the clock owes an
/// `active` import is the import suite's rule, and a row about cancelling one must not race
/// it.
async fn started_image_task(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    disks: usize,
) -> String {
    let mut params: Params = vec![p("Description", format!("the import of {case}"))];
    for index in 1..=disks {
        params.push(p(&format!("DiskContainer.{index}.Format"), "VMDK"));
        params.push(p(
            &format!("DiskContainer.{index}.Url"),
            format!("s3://disk-images/{case}-{index}.vmdk"),
        ));
    }
    let (status, body) = call(router, REGION, "ImportImage", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ImportImage, got {status} with body {body}"
    );
    let import_task_id = element(&body, "importTaskId")
        .unwrap_or_else(|| panic!("case {case:?}: no importTaskId in {body}"));
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .import_image_tasks
        .get_mut(&import_task_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds the task {import_task_id}"))
        .completed_at = Instant::now() + BEYOND_THIS_TEST;
    import_task_id
}

/// Store one import image task in `state`, with the members an import carries filled in so
/// an assertion that the cancel moved only the state has something to compare, and answer
/// the id it was minted under.
fn seed_image_task(engine: &engine::InMemoryEngine, state: Option<&str>, disks: usize) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let import_task_id = guard.next_id(IMPORT_IMAGE_TASK_ID_PREFIX);
    let snapshot_details = (0..disks)
        .map(|index| SnapshotDetail {
            description: Some(format!("disk {index}")),
            device_name: Some(format!("/dev/xvd{}", index + 1)),
            disk_image_size: Some(8.0),
            format: Some("VMDK".to_string()),
            url: Some(format!("s3://disk-images/{import_task_id}-{index}.vmdk")),
            status: state.map(str::to_string),
            ..SnapshotDetail::default()
        })
        .collect();
    let now = Instant::now();
    guard.import_image_tasks.insert(
        import_task_id.clone(),
        ImportImageTaskRecord {
            view: ImportImageTask {
                import_task_id: Some(import_task_id.clone()),
                description: Some("a seeded import".to_string()),
                architecture: Some("x86_64".to_string()),
                hypervisor: Some("xen".to_string()),
                encrypted: Some(false),
                snapshot_details: Some(snapshot_details),
                status: state.map(str::to_string),
                status_message: Some("converting".to_string()),
                tags: Some(vec![Tag {
                    key: Some("Name".to_string()),
                    value: Some("seeded".to_string()),
                }]),
                ..ImportImageTask::default()
            },
            started_at: now,
            completed_at: now + BEYOND_THIS_TEST,
        },
    );
    import_task_id
}

/// Store one import snapshot task in `state`, through the one writer of one, and answer the
/// id it was minted under.
fn seed_snapshot_task(engine: &engine::InMemoryEngine, state: Option<&str>) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .insert_import_snapshot_task(ImportSnapshotTaskSpec {
            description: Some("a seeded snapshot import".to_string()),
            detail: SnapshotTaskDetail {
                status: state.map(str::to_string),
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
            tags: vec![Tag {
                key: Some("Name".to_string()),
                value: Some("seeded".to_string()),
            }],
        })
        .expect("the store must store the import snapshot task")
}

/// Every import task id the store holds, of either kind, in its own order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .import_image_tasks
        .keys()
        .chain(guard.import_snapshot_tasks.keys())
        .cloned()
        .collect()
}

/// Whether the clock owes `import_task_id` the end of a cancellation.
fn cancellation_owed(engine: &engine::InMemoryEngine, import_task_id: &str) -> bool {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard.import_task_deleted_at.contains_key(import_task_id)
}

// ---------------------------------------------------------------------------
// RULE 1: the id's KIND is read before any lookup
// ---------------------------------------------------------------------------

/// Values that name NO kind of import task: each row departs in one way from an id of a
/// kind — a digit outside lowercase hexadecimal, an uppercase suffix, an empty suffix, a
/// missing or unknown kind segment, another resource's id, and a value of no resource-id
/// shape at all. Generated over BOTH minting prefixes, so neither kind's form is asserted
/// only through the other's.
fn malformed_id_rows() -> Vec<(String, String)> {
    let departures = [
        ("an uppercase suffix", "0123ABCD"),
        ("a suffix that is not hexadecimal", "0123abcg"),
        ("a suffix that is not hexadecimal at its end", "0123456789abcdefg"),
        ("an empty suffix", ""),
    ];
    let mut rows: Vec<(String, String)> = Vec::new();
    for prefix in [
        IMPORT_IMAGE_TASK_ID_PREFIX_WITH_HYPHEN,
        IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN,
    ] {
        for (case, suffix) in departures {
            rows.push((
                format!("a {prefix} id with {case}"),
                format!("{prefix}{suffix}"),
            ));
        }
    }
    for (case, id) in [
        ("no kind segment", "import-fffffff0"),
        ("a kind EC2 issues no import task under", "import-vol-0123abcd"),
        ("another resource's id", "export-i-0123abcd"),
        ("a value of no resource-id shape", "not an import task"),
    ] {
        rows.push((case.to_string(), id.to_string()));
    }
    rows
}

/// Ids of a kind EC2 could have issued a task under, which the account holds no task for:
/// each kind's minting prefix crossed with the minted suffix lengths and with the suffix
/// lengths OUTSIDE them — the length of the suffix is no part of what the parameter admits,
/// so an id is reported as the task of its kind that is not there either way. Each row
/// carries the kind a refusal naming it must spell.
fn minted_id_rows() -> Vec<(String, String, &'static str)> {
    let mut rows = Vec::new();
    for kind in [Kind::Image, Kind::Snapshot] {
        for (length, suffix) in MINTED_SUFFIXES.iter().chain(UNMINTED_SUFFIXES.iter()) {
            rows.push((
                format!("a {kind:?} task id in {length}"),
                format!("{}{suffix}", kind.id_prefix()),
                kind.resource_type(),
            ));
        }
    }
    rows
}

/// Rule 1: a value naming no kind of import task is refused `InvalidConversionTaskId` with
/// the message naming the value it was sent, while an id whose kind EC2 reads that the
/// account holds no task for is refused under the same code with the message naming that
/// kind — and neither refusal writes anything.
#[tokio::test]
async fn an_id_ec2_could_not_have_issued_is_refused_before_any_lookup() {
    let (router, engine) = emulator();

    for (case, import_task_id) in malformed_id_rows() {
        assert!(
            !import_image_task_id_is_well_formed(&import_task_id)
                && !import_snapshot_task_id_is_well_formed(&import_task_id),
            "case {case:?}: {import_task_id} must not read as a minted import task id"
        );
        let (status, body) = cancel_task(&router, &import_task_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_CONVERSION_TASK_ID,
            "case {case:?}: expected {INVALID_CONVERSION_TASK_ID}, got {body}"
        );
        // The message EC2 builds for a value it cannot read as an id of the kind the
        // parameter names repeats that value back. The XML body escapes the quotes around
        // it, so the text is compared.
        let message = element(&body, "Message").map(|message| unescaped(&message));
        assert_eq!(
            message,
            Some(format!("Invalid id: \"{import_task_id}\"")),
            "case {case:?}: expected the malformed-id message to name the id, got {message:?}"
        );
    }

    for (case, import_task_id, resource_type) in minted_id_rows() {
        let (status, body) = cancel_task(&router, &import_task_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_CONVERSION_TASK_ID,
            "case {case:?}: expected {INVALID_CONVERSION_TASK_ID}, got {body}"
        );
        let message = element(&body, "Message").map(|message| unescaped(&message));
        assert_eq!(
            message,
            Some(format!(
                "The {resource_type} ID '{import_task_id}' does not exist"
            )),
            "case {case:?}: expected the not-found message to name the kind and the id, got \
             {message:?}"
        );
    }

    let held = stored_ids(&engine);
    assert!(
        held.is_empty(),
        "a refused cancel stores nothing, so the account still holds no import task, got \
         {held:?}"
    );
}

/// Rule 1: a request that names no task at all names nothing to cancel, so it is refused
/// `MissingParameter` rather than acted on — `ImportTaskId` is not modelled `@required`, so
/// the handler is what refuses it.
#[tokio::test]
async fn a_cancel_that_names_no_task_is_refused_as_a_missing_parameter() {
    let (router, _engine) = emulator();
    // `(case, the parameters under test)`: an absent field, and one an ec2Query body cannot
    // tell from absent.
    let rows: Vec<(&str, Params)> = vec![
        ("no ImportTaskId field at all", Vec::new()),
        ("an empty ImportTaskId", vec![p("ImportTaskId", "")]),
        (
            "an empty ImportTaskId beside a cancel reason",
            vec![p("ImportTaskId", ""), p("CancelReason", "no longer needed")],
        ),
    ];
    for (case, params) in rows {
        let (status, body) = cancel(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            MISSING_PARAMETER,
            "case {case:?}: expected {MISSING_PARAMETER}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: an admitted cancel moves the task into `deleting`, the clock finishes it
// ---------------------------------------------------------------------------

/// One row of the admitted-cancel table: the case, the kind of task, how many disks an
/// import image task converts, and the reason the cancel names (if any).
struct CancelRow {
    case: &'static str,
    kind: Kind,
    disks: usize,
    reason: Option<String>,
}

/// The admitted-cancel rows: both kinds of task, imports of one and of several disks, a
/// cancel with a reason and one without, and a reason at the width this service publishes.
fn cancel_rows() -> Vec<CancelRow> {
    vec![
        CancelRow {
            case: "one-disk-image-import-with-a-reason",
            kind: Kind::Image,
            disks: 1,
            reason: Some("the wrong disk image was uploaded".to_string()),
        },
        CancelRow {
            case: "three-disk-image-import-without-a-reason",
            kind: Kind::Image,
            disks: 3,
            reason: None,
        },
        CancelRow {
            case: "two-disk-image-import-with-a-reason-at-the-published-width",
            kind: Kind::Image,
            disks: 2,
            reason: Some("r".repeat(MAX_IMPORT_TASK_CANCEL_REASON_CHARS)),
        },
        CancelRow {
            case: "snapshot-import-with-a-reason",
            kind: Kind::Snapshot,
            disks: 0,
            reason: Some("cancelled by the nightly job".to_string()),
        },
        CancelRow {
            case: "snapshot-import-without-a-reason",
            kind: Kind::Snapshot,
            disks: 0,
            reason: None,
        },
    ]
}

/// Rule 3: a cancel of an `active` task of either kind is admitted, reports the state the
/// task was in beside the `deleting` it is in now, carries the reason the request gave onto
/// the task, and only the CLOCK takes it to `deleted`.
#[tokio::test]
async fn an_active_import_task_of_either_kind_cancels_and_finishes_on_the_clock() {
    let (router, engine) = emulator();
    let rows = cancel_rows();
    let mut ids: Vec<(&CancelRow, String)> = Vec::new();
    for row in &rows {
        let import_task_id = match row.kind {
            Kind::Image => started_image_task(&router, &engine, row.case, row.disks).await,
            Kind::Snapshot => seed_snapshot_task(&engine, Some(IMPORT_TASK_STATE_ACTIVE)),
        };
        let before = reported(&router, row.case, row.kind, &import_task_id).await;
        assert_eq!(
            before.state.as_deref(),
            Some(IMPORT_TASK_STATE_ACTIVE),
            "case {:?}: the row's task must start {IMPORT_TASK_STATE_ACTIVE}, got {before:?}",
            row.case
        );
        ids.push((row, import_task_id));
    }

    for (row, import_task_id) in &ids {
        let mut params = vec![p("ImportTaskId", import_task_id.as_str())];
        if let Some(reason) = row.reason.as_deref() {
            params.push(p("CancelReason", reason));
        }
        let (status, body) = cancel(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {body}",
            row.case
        );
        assert_eq!(
            member(&body, "importTaskId").as_deref(),
            Some(import_task_id.as_str()),
            "case {:?}: the response names the task it cancelled, got {body}",
            row.case
        );
        assert_eq!(
            member(&body, "previousState").as_deref(),
            Some(IMPORT_TASK_STATE_ACTIVE),
            "case {:?}: the response reports the state the task was in, got {body}",
            row.case
        );
        assert_eq!(
            member(&body, "state").as_deref(),
            Some(IMPORT_TASK_STATE_DELETING),
            "case {:?}: the response reports the state the task is in now, got {body}",
            row.case
        );

        let after = reported(&router, row.case, row.kind, import_task_id).await;
        assert_eq!(
            after.state.as_deref(),
            Some(IMPORT_TASK_STATE_DELETING),
            "case {:?}: straight after the cancel the task is {IMPORT_TASK_STATE_DELETING}, \
             got {after:?}",
            row.case
        );
        assert_eq!(
            after.status_message, row.reason,
            "case {:?}: the task reports the reason the cancel gave, got {after:?}",
            row.case
        );
        let expected_disks: Vec<Option<String>> =
            vec![Some(IMPORT_TASK_STATE_DELETING.to_string()); row.disks];
        assert_eq!(
            after.disk_states, expected_disks,
            "case {:?}: every disk of the import stops where the task did, got {after:?}",
            row.case
        );
        assert!(
            cancellation_owed(&engine, import_task_id),
            "case {:?}: the clock owes the cancelled task its {IMPORT_TASK_STATE_DELETED} \
             state",
            row.case
        );
    }

    past_the_transition().await;

    for (row, import_task_id) in &ids {
        let settled = reported(&router, row.case, row.kind, import_task_id).await;
        assert_eq!(
            settled.state.as_deref(),
            Some(IMPORT_TASK_STATE_DELETED),
            "case {:?}: once the cancellation falls due the task is \
             {IMPORT_TASK_STATE_DELETED}, got {settled:?}",
            row.case
        );
        assert_eq!(
            settled.status_message, row.reason,
            "case {:?}: the reason survives the move to {IMPORT_TASK_STATE_DELETED}, got \
             {settled:?}",
            row.case
        );
        let expected_disks: Vec<Option<String>> =
            vec![Some(IMPORT_TASK_STATE_DELETED.to_string()); row.disks];
        assert_eq!(
            settled.disk_states, expected_disks,
            "case {:?}: every disk of the import ends where the task did, got {settled:?}",
            row.case
        );
        assert!(
            !cancellation_owed(&engine, import_task_id),
            "case {:?}: a cancellation the clock has applied is owed no longer",
            row.case
        );
    }
}

/// Rule 3: a cancel moves the task it names and nothing else — the account's other tasks
/// keep the state they were in, of either kind, and every record stays in the store, since
/// a state is only reportable while the task it belongs to is there.
#[tokio::test]
async fn a_cancel_touches_only_the_task_it_names() {
    let (router, engine) = emulator();
    let cancelled = started_image_task(&router, &engine, "the-cancelled-import", 1).await;
    let other_image = started_image_task(&router, &engine, "another-import", 2).await;
    let other_snapshot = seed_snapshot_task(&engine, Some(IMPORT_TASK_STATE_ACTIVE));
    let completed = seed_image_task(&engine, Some(IMPORT_TASK_STATE_COMPLETED), 1);

    let (status, body) = cancel_task(&router, &cancelled).await;
    assert_eq!(
        status, 200,
        "cancelling one of four tasks: expected HTTP 200, got {status} with body {body}"
    );

    // `(case, the kind, the task, the state it must be in after the cancel of another)`.
    let expected = [
        (
            "the task the cancel named",
            Kind::Image,
            &cancelled,
            IMPORT_TASK_STATE_DELETING,
        ),
        (
            "another active import image task",
            Kind::Image,
            &other_image,
            IMPORT_TASK_STATE_ACTIVE,
        ),
        (
            "an active import snapshot task",
            Kind::Snapshot,
            &other_snapshot,
            IMPORT_TASK_STATE_ACTIVE,
        ),
        (
            "an import image task that had completed",
            Kind::Image,
            &completed,
            IMPORT_TASK_STATE_COMPLETED,
        ),
    ];
    for (case, kind, import_task_id, expected_state) in expected {
        let task = reported(&router, case, kind, import_task_id).await;
        assert_eq!(
            task.state.as_deref(),
            Some(expected_state),
            "case {case:?}: expected {expected_state}, got {task:?}"
        );
    }

    let mut held = stored_ids(&engine);
    held.sort();
    let mut expected_ids = vec![
        cancelled.clone(),
        other_image.clone(),
        other_snapshot.clone(),
        completed.clone(),
    ];
    expected_ids.sort();
    assert_eq!(
        held, expected_ids,
        "a cancel neither removes a record nor re-keys one, so every task stays reportable \
         under the id it was stored under"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: a cancel the task's state does not admit is refused and writes nothing
// ---------------------------------------------------------------------------

/// Rule 2: a task that is not importing any more has nothing left to cancel, so the cancel
/// is refused `IncorrectState` naming the state it IS in, and leaves that state alone — over
/// every non-`active` state of the import-task catalog, for both kinds, plus a task carrying
/// no state at all.
#[tokio::test]
async fn a_cancel_the_tasks_state_does_not_admit_changes_nothing() {
    let (router, engine) = emulator();
    // `(case, the kind, the state the task is seeded in)`. `None` is a task carrying no
    // status at all, which is no state of the catalog and so admits no cancel either.
    let rows: Vec<(&str, Kind, Option<&str>)> = vec![
        (
            "an import image task that has completed",
            Kind::Image,
            Some(IMPORT_TASK_STATE_COMPLETED),
        ),
        (
            "an import image task a cancel is working through",
            Kind::Image,
            Some(IMPORT_TASK_STATE_DELETING),
        ),
        (
            "an import image task a cancel has finished with",
            Kind::Image,
            Some(IMPORT_TASK_STATE_DELETED),
        ),
        ("an import image task carrying no state", Kind::Image, None),
        (
            "an import snapshot task that has completed",
            Kind::Snapshot,
            Some(IMPORT_TASK_STATE_COMPLETED),
        ),
        (
            "an import snapshot task a cancel is working through",
            Kind::Snapshot,
            Some(IMPORT_TASK_STATE_DELETING),
        ),
        (
            "an import snapshot task a cancel has finished with",
            Kind::Snapshot,
            Some(IMPORT_TASK_STATE_DELETED),
        ),
        (
            "an import snapshot task carrying no state",
            Kind::Snapshot,
            None,
        ),
    ];

    for (case, kind, state) in rows {
        let import_task_id = match kind {
            Kind::Image => seed_image_task(&engine, state, 2),
            Kind::Snapshot => seed_snapshot_task(&engine, state),
        };
        let before = reported(&router, case, kind, &import_task_id).await;
        assert_eq!(
            before.state.as_deref(),
            state,
            "case {case:?}: the row's task must start in {state:?}, got {before:?}"
        );

        let (status, body) = cancel_task(&router, &import_task_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INCORRECT_STATE,
            "case {case:?}: expected {INCORRECT_STATE}, got {body}"
        );
        let message = element(&body, "Message").map(|message| unescaped(&message));
        assert_eq!(
            message,
            Some(format!(
                "Import task '{import_task_id}' is in the '{}' state.",
                state.unwrap_or_default()
            )),
            "case {case:?}: the refusal names the task and the state it is in, got {message:?}"
        );

        let after = reported(&router, case, kind, &import_task_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused cancel leaves the task exactly as it was, got {after:?}"
        );
        assert!(
            !cancellation_owed(&engine, &import_task_id),
            "case {case:?}: a refused cancel starts no cancellation"
        );
    }
}

/// Rule 2 and rule 3 together: a cancel of a task whose cancellation is already working
/// through is refused, so one cancel is not undone or restarted by the next.
#[tokio::test]
async fn a_second_cancel_of_one_task_is_refused() {
    let (router, engine) = emulator();
    let import_task_id = started_image_task(&router, &engine, "the-twice-cancelled", 1).await;

    let (status, body) = cancel_task(&router, &import_task_id).await;
    assert_eq!(
        status, 200,
        "the first cancel must be admitted, got {status} with body {body}"
    );
    let first_due = {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        *guard
            .import_task_deleted_at
            .get(&import_task_id)
            .expect("the first cancel starts a cancellation")
    };

    let (status, body) = cancel_task(&router, &import_task_id).await;
    assert_eq!(
        status, 400,
        "the second cancel must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INCORRECT_STATE,
        "the second cancel must be refused {INCORRECT_STATE}, got {body}"
    );
    let second_due = {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        guard.import_task_deleted_at.get(&import_task_id).copied()
    };
    assert_eq!(
        second_due,
        Some(first_due),
        "a refused cancel does not move when the first cancellation falls due"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: DryRun answers what the request would have done, and writes nothing
// ---------------------------------------------------------------------------

/// Rule 4: `DryRun` reports whether the request WOULD have worked — `DryRunOperation` for a
/// cancel that would have been admitted, and otherwise the very refusal the request would
/// have been refused with — and never moves a task.
#[tokio::test]
async fn a_dry_run_cancel_reports_what_would_have_happened_and_writes_nothing() {
    let (router, engine) = emulator();
    let active_image = started_image_task(&router, &engine, "the-dry-run-import", 2).await;
    let active_snapshot = seed_snapshot_task(&engine, Some(IMPORT_TASK_STATE_ACTIVE));
    let completed = seed_image_task(&engine, Some(IMPORT_TASK_STATE_COMPLETED), 1);

    // `(case, the task id, the status and code the dry run must answer)`.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "an active import image task",
            active_image.clone(),
            412,
            DRY_RUN_OPERATION,
        ),
        (
            "an active import snapshot task",
            active_snapshot.clone(),
            412,
            DRY_RUN_OPERATION,
        ),
        (
            "a completed import image task",
            completed.clone(),
            400,
            INCORRECT_STATE,
        ),
        (
            "an id EC2 could not have issued",
            "import-fffffff0".to_string(),
            400,
            INVALID_CONVERSION_TASK_ID,
        ),
        (
            "an id EC2 could have issued that names no task",
            format!("{IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN}0123abcd"),
            400,
            INVALID_CONVERSION_TASK_ID,
        ),
    ];

    for (case, import_task_id, expected_status, expected_code) in &rows {
        let (status, body) = cancel(
            &router,
            &[
                p("ImportTaskId", import_task_id.as_str()),
                p("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // `(case, the kind, the task, the state a dry run must have left it in)`.
    let untouched = [
        (
            "the active import image task",
            Kind::Image,
            &active_image,
            IMPORT_TASK_STATE_ACTIVE,
        ),
        (
            "the active import snapshot task",
            Kind::Snapshot,
            &active_snapshot,
            IMPORT_TASK_STATE_ACTIVE,
        ),
        (
            "the completed import image task",
            Kind::Image,
            &completed,
            IMPORT_TASK_STATE_COMPLETED,
        ),
    ];
    for (case, kind, import_task_id, expected_state) in untouched {
        let task = reported(&router, case, kind, import_task_id).await;
        assert_eq!(
            task.state.as_deref(),
            Some(expected_state),
            "case {case:?}: a dry run leaves the task in {expected_state}, got {task:?}"
        );
        assert!(
            !cancellation_owed(&engine, import_task_id),
            "case {case:?}: a dry run starts no cancellation"
        );
    }
}

/// A cancel reason past the width this service keeps is refused `InvalidParameterValue`
/// before anything is read or written, so an unbounded member cannot grow the store: the row
/// at the bound itself is admitted by
/// [`an_active_import_task_of_either_kind_cancels_and_finishes_on_the_clock`].
#[tokio::test]
async fn a_cancel_reason_past_the_published_width_is_refused() {
    let (router, engine) = emulator();
    let import_task_id = started_image_task(&router, &engine, "the-long-reason-import", 1).await;
    let reason = "r".repeat(MAX_IMPORT_TASK_CANCEL_REASON_CHARS + 1);

    let (status, body) = cancel(
        &router,
        &[
            p("ImportTaskId", import_task_id.as_str()),
            p("CancelReason", reason),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a reason past the width must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "a reason past the width must be refused {INVALID_PARAMETER_VALUE}, got {body}"
    );

    let task = reported(&router, "the refused cancel", Kind::Image, &import_task_id).await;
    assert_eq!(
        task.state.as_deref(),
        Some(IMPORT_TASK_STATE_ACTIVE),
        "a refused cancel leaves the task {IMPORT_TASK_STATE_ACTIVE}, got {task:?}"
    );
    assert!(
        !cancellation_owed(&engine, &import_task_id),
        "a refused cancel starts no cancellation"
    );
}
