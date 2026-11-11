// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the AMI export slice — `ExportImage`, `DescribeExportImageTasks` and
//! the cancellation of an image export task — driven over the wire the service actually
//! speaks: `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `ExportImage` STORES A TASK for the AMI, the format and the destination the request
//!    names, and `DescribeExportImageTasks` REPORTS THE STORE: every member round-trips
//!    through a later read, the task id is one this service MINTED (`export-ami-<hex>`), and
//!    the members the service owns — the status and the percentage of progress — are derived
//!    rather than taken from the request;
//! 2. the CLOCK, not a caller, moves a task `active` -> `completed`, and a cancellation
//!    `deleting` -> `deleted`. A read reports the move; it never performs one, and no second
//!    call is needed to make it happen;
//! 3. `ExportImageTaskIds` NARROWS the report, and an id the account holds no task for selects
//!    no row rather than failing the call;
//! 4. `Filters` select with EC2's AND/OR rule — "If you specify multiple filters, the filters
//!    are joined with an AND" and, within one filter, the values with an OR (`Filter`,
//!    `Filter$Values`) — over the one name this operation documents (`task-state`) and the four
//!    values `ExportImageTask$Status` publishes; a name it does not document selects nothing
//!    rather than being refused, which is what the paired operation carrying the identical
//!    documented sentence was recorded doing;
//! 5. `MaxResults` and `NextToken` page the report in `ExportImageTaskId` order: a walk reports
//!    every selected task exactly once, in ascending id order, the last page carries no token,
//!    and a token this service did not mint is refused;
//! 6. a request EC2 will not accept is refused with EC2's own code and NOTHING is stored: a
//!    required member left out, a description past its published 255-character bound, a bucket
//!    name outside Amazon S3's naming rules, an AMI id the account cannot see — and a
//!    `ClientToken` replay reports the task it already started instead of a second one.
//!
//! Each table row is a distinct input along what the rule generalises over — which disk image
//! format, which bucket name form, whether a prefix, a description, a role or tags are given
//! and how many, which ids narrow the report, which status is filtered for and how many values,
//! which page size, and which bound and which side of it — so no row is the specification.

use std::time::{Duration, Instant};

use ec2::ops::types::DISK_IMAGE_FORMAT_VALUES;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, EXPORT_IMAGE_TASK_STATUS_ACTIVE,
    EXPORT_IMAGE_TASK_STATUS_COMPLETED, EXPORT_IMAGE_TASK_STATUS_DELETED,
    EXPORT_IMAGE_TASK_STATUS_DELETING, EXPORT_IMAGE_TASK_STATUS_VALUES, INCORRECT_STATE,
    INVALID_AMI_ID_MALFORMED, INVALID_AMI_ID_NOT_FOUND, INVALID_PAGINATION_TOKEN,
    INVALID_PARAMETER_VALUE, INVALID_REQUEST, ImageSpec, MAX_ACTIVE_IMPORT_PROGRESS,
    MAX_RESOURCE_IDS, MAX_S3_BUCKET_NAME_CHARS, MIN_S3_BUCKET_NAME_CHARS, MISSING_PARAMETER,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, filters, indexed};
use super::wire::{child, child_text, document, is_long_id, set_items};

/// The Region every call here is signed for. An export image task carries no Region member, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The id prefix an export image task carries, from the crate's one definition of it.
const EXPORT_IMAGE_TASK_ID_PREFIX: &str = ec2::ops::cancel_export_task::IMAGE_EXPORT_TASK_ID_PREFIX;

/// "A description of the image being exported. The maximum length is 255 characters"
/// (`ExportImageRequest$Description`): the published bound this suite tests both sides of,
/// read from the handler's own constant so the two cannot drift.
const MAX_DESCRIPTION_CHARS: usize = ec2::ops::export_image::MAX_EXPORT_DESCRIPTION_CHARS;

/// A well-formed AMI id no image of the account is stored under.
const UNKNOWN_IMAGE_ID: &str = "ami-0ffffffffffffffff";

/// Something that is not an AMI id at all.
const MALFORMED_IMAGE_ID: &str = "ami-not-an-id";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call through the shared harness.
async fn api(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `DescribeExportImageTasks` call.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(router, "DescribeExportImageTasks", params).await
}

/// One `CancelExportTask` call for `export_task_id`.
async fn cancel(router: &MultiServiceRouter, export_task_id: &str) -> (u16, String) {
    api(
        router,
        "CancelExportTask",
        &[p("ExportTaskId", export_task_id)],
    )
    .await
}

/// The `ExportImageTaskId.N` form fields of a narrowing request.
fn ids(values: &[&str]) -> Vec<(String, String)> {
    indexed("ExportImageTaskId", values)
}

/// Wait until every clock-owed transition started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// One export a row asks for: what varies between the rows of a table.
#[derive(Debug, Clone)]
struct ExportSpec<'a> {
    image_id: &'a str,
    disk_image_format: &'a str,
    s3_bucket: &'a str,
    s3_prefix: Option<&'a str>,
    description: Option<&'a str>,
    role_name: Option<&'a str>,
    tags: &'a [(&'a str, &'a str)],
    /// The `ResourceType` the tag specification names — the task this action creates, unless a
    /// row is asking what happens when it names anything else.
    tag_resource_type: &'a str,
    client_token: Option<&'a str>,
}

impl<'a> ExportSpec<'a> {
    /// An export of `image_id` in the first published disk image format into `bucket`, with
    /// every optional member left out — what a row varies FROM.
    fn new(image_id: &'a str, disk_image_format: &'a str, s3_bucket: &'a str) -> Self {
        Self {
            image_id,
            disk_image_format,
            s3_bucket,
            s3_prefix: None,
            description: None,
            role_name: None,
            tags: &[],
            tag_resource_type: "export-image-task",
            client_token: None,
        }
    }

    /// This spec as the ec2Query form fields of an `ExportImage` request.
    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![
            p("ImageId", self.image_id),
            p("DiskImageFormat", self.disk_image_format),
            p("S3ExportLocation.S3Bucket", self.s3_bucket),
        ];
        if let Some(prefix) = self.s3_prefix {
            params.push(p("S3ExportLocation.S3Prefix", prefix));
        }
        if let Some(description) = self.description {
            params.push(p("Description", description));
        }
        if let Some(role_name) = self.role_name {
            params.push(p("RoleName", role_name));
        }
        if let Some(token) = self.client_token {
            params.push(p("ClientToken", token));
        }
        if !self.tags.is_empty() {
            params.push(p("TagSpecification.1.ResourceType", self.tag_resource_type));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(p(&format!("TagSpecification.1.Tag.{position}.Key"), *key));
            params.push(p(&format!("TagSpecification.1.Tag.{position}.Value"), *value));
        }
        params
    }
}

/// One `ExportImage` call for `spec`.
async fn export(router: &MultiServiceRouter, spec: &ExportSpec<'_>) -> (u16, String) {
    api(router, "ExportImage", &spec.params()).await
}

/// One `ExportImage` call that must succeed, answering the task id it minted.
async fn export_ok(router: &MultiServiceRouter, case: &str, spec: &ExportSpec<'_>) -> String {
    let (status, body) = export(router, spec).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ExportImage, got {status} with body {body}"
    );
    element(&body, "exportImageTaskId")
        .unwrap_or_else(|| panic!("case {case:?}: no <exportImageTaskId> in {body}"))
}

/// Write one AMI of the calling account into the engine's store, the way a register operation
/// would, and report the id it was given.
fn seed_image(engine: &engine::InMemoryEngine, name: &str) -> String {
    let state = engine.state::<Ec2State>();
    let mut guard = state.lock();
    guard.insert_image(ImageSpec {
        name: name.to_string(),
        ..ImageSpec::default()
    })
}

/// How many export image tasks the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().export_image_tasks.len()
}

/// The status the STORE holds for `export_task_id` right now, read straight off the record so a
/// test can tell a reported status from a stored one.
fn stored_status(engine: &engine::InMemoryEngine, export_task_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .export_image_tasks
        .get(export_task_id)
        .map(|record| record.status_at(Instant::now()).to_string())
        .unwrap_or_else(|| panic!("the store holds the export image task {export_task_id}"))
}

/// A bucket name of exactly `length` characters that satisfies the Amazon S3 bucket naming
/// rules: lowercase letters only, so it begins and ends with a letter whatever the length.
fn bucket_of_length(length: usize) -> String {
    "a".repeat(length)
}

/// One export image task as a response reported it: every member flattened to the text on the
/// wire, so what a read wrote can be compared against what the create asked for.
///
/// `progress` and `s3_prefix` stay optional because their ABSENCE is part of the rule: a task
/// that is no longer running reports no percentage, and a request that named no prefix stores
/// none.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    export_image_task_id: String,
    description: String,
    image_id: String,
    s3_bucket: String,
    s3_prefix: Option<String>,
    status: String,
    status_message: Option<String>,
    progress: Option<String>,
    tags: Vec<(String, String)>,
}

/// Every export image task a response carries, in the order it reported them.
///
/// The element carrying an `<exportImageTaskId>` child is the task, which makes this one reader
/// for both envelopes: `DescribeExportImageTasks` nests its tasks in `<exportImageTaskSet>`,
/// `ExportImage` answers with the task's members on the envelope itself.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "exportImageTaskId").is_some())
        .map(|item| {
            let location = child(item, "s3ExportLocation");
            Reported {
                export_image_task_id: child_text(item, "exportImageTaskId").unwrap_or_default(),
                description: child_text(item, "description").unwrap_or_default(),
                image_id: child_text(item, "imageId").unwrap_or_default(),
                s3_bucket: location
                    .and_then(|inner| child_text(inner, "s3Bucket"))
                    .unwrap_or_default(),
                s3_prefix: location.and_then(|inner| child_text(inner, "s3Prefix")),
                status: child_text(item, "status").unwrap_or_default(),
                status_message: child_text(item, "statusMessage"),
                progress: child_text(item, "progress"),
                tags: set_items(item, "tagSet")
                    .into_iter()
                    .map(|tag| {
                        (
                            child_text(tag, "key").unwrap_or_default(),
                            child_text(tag, "value").unwrap_or_default(),
                        )
                    })
                    .collect(),
            }
        })
        .collect()
}

/// The one export image task a response carries, or a panic naming the case.
fn reported_one(case: &str, body: &str) -> Reported {
    let mut found = reported(case, body);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one export image task, got {found:?} from {body}"
    );
    found.remove(0)
}

/// Every `exportImageTaskId` a response reported, in the order it reported them.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|task| task.export_image_task_id)
        .collect()
}

/// The ids `params` selects, out of a call that must succeed.
async fn selected(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let mut found = reported_ids(case, &body);
    found.sort();
    found
}

/// The table rules 1 and 3 work from: one export per row, each row a distinct combination of
/// what an export request can carry.
///
/// Between them the rows name every value of the published `DiskImageFormat` catalog
/// (`ec2::ops::types::DISK_IMAGE_FORMAT_VALUES`, generated from the model), which
/// [`the_rows_name_every_published_disk_image_format`] pins, each bucket-name form the Amazon S3
/// rules admit at both published length bounds, a prefix given and left out, a description given
/// and left out, a role given and left out, and zero, one and two tags.
fn rows() -> Vec<(&'static str, ExportSpec<'static>)> {
    let longest_bucket: &'static str =
        Box::leak(bucket_of_length(MAX_S3_BUCKET_NAME_CHARS).into_boxed_str());
    let shortest_bucket: &'static str =
        Box::leak(bucket_of_length(MIN_S3_BUCKET_NAME_CHARS).into_boxed_str());
    let longest_description: &'static str =
        Box::leak("d".repeat(MAX_DESCRIPTION_CHARS).into_boxed_str());
    vec![
        (
            "a VMDK export with nothing optional given",
            ExportSpec::new("", "VMDK", "vm-exports"),
        ),
        (
            "a RAW export into a prefix, described, with one tag",
            ExportSpec {
                s3_prefix: Some("exports/2026/"),
                description: Some("nightly AMI export"),
                tags: &[("purpose", "disaster-recovery")],
                ..ExportSpec::new("", "RAW", "vm.exports.example")
            },
        ),
        (
            "a VHD export under a named role, with two tags",
            ExportSpec {
                s3_prefix: Some("vhd/"),
                role_name: Some("my-vmexport-role"),
                tags: &[("owner", "platform"), ("cost-centre", "4711")],
                ..ExportSpec::new("", "VHD", "20261022-exports")
            },
        ),
        (
            "an export into the shortest bucket name the rules allow",
            ExportSpec {
                description: Some(""),
                ..ExportSpec::new("", "VMDK", shortest_bucket)
            },
        ),
        (
            "an export into the longest bucket name the rules allow",
            ExportSpec {
                description: Some(longest_description),
                ..ExportSpec::new("", "RAW", longest_bucket)
            },
        ),
    ]
}

/// RULE 1: the rows between them name every published disk image format, so no row is the
/// specification and a format added to the model fails this rather than going untested.
#[test]
fn the_rows_name_every_published_disk_image_format() {
    let mut named: Vec<&str> = rows()
        .iter()
        .map(|(_, spec)| spec.disk_image_format)
        .collect();
    named.sort_unstable();
    named.dedup();
    let mut published: Vec<&str> = DISK_IMAGE_FORMAT_VALUES.to_vec();
    published.sort_unstable();
    assert_eq!(
        named, published,
        "the table must name every published DiskImageFormat, got {named:?} for {published:?}"
    );
}

/// RULE 1: an export stores a task for what the request named, the create reports it, and a
/// later describe reports the same task — the id minted, the status and the progress derived.
#[tokio::test]
async fn an_export_stores_a_task_and_the_describe_reports_it() {
    let (router, engine) = super::ec2_query::emulator();
    let table = rows();

    let mut expected: Vec<(&str, Reported, String)> = Vec::new();
    for (case, spec) in &table {
        let image_id = seed_image(&engine, case);
        let spec = ExportSpec {
            image_id: &image_id,
            ..spec.clone()
        };
        let (status, body) = export(&router, &spec).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ExportImage, got {status} with body {body}"
        );
        let created = reported_one(case, &body);

        // The id is one this service MINTED, not anything the request carried.
        assert!(
            is_long_id(EXPORT_IMAGE_TASK_ID_PREFIX, &created.export_image_task_id),
            "case {case:?}: expected a minted {EXPORT_IMAGE_TASK_ID_PREFIX}-<hex> id, \
             got {:?}",
            created.export_image_task_id
        );
        // The members the REQUEST named round-trip, as given.
        assert_eq!(
            created.image_id, image_id,
            "case {case:?}: expected the exported AMI {image_id}, got {:?}",
            created.image_id
        );
        assert_eq!(
            created.s3_bucket, spec.s3_bucket,
            "case {case:?}: expected the bucket {:?}, got {:?}",
            spec.s3_bucket, created.s3_bucket
        );
        assert_eq!(
            created.s3_prefix.as_deref(),
            spec.s3_prefix,
            "case {case:?}: expected the prefix {:?}, got {:?}",
            spec.s3_prefix,
            created.s3_prefix
        );
        assert_eq!(
            created.description,
            spec.description.unwrap_or_default(),
            "case {case:?}: expected the description {:?}, got {:?}",
            spec.description,
            created.description
        );
        let expected_tags: Vec<(String, String)> = spec
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            created.tags, expected_tags,
            "case {case:?}: expected the tags {expected_tags:?}, got {:?}",
            created.tags
        );
        // The two members only `ExportImageResult` carries come back as the request named them.
        assert_eq!(
            element(&body, "diskImageFormat").as_deref(),
            Some(spec.disk_image_format),
            "case {case:?}: expected the disk image format {:?} in {body}",
            spec.disk_image_format
        );
        assert_eq!(
            element(&body, "roleName").as_deref(),
            spec.role_name,
            "case {case:?}: expected the role {:?} in {body}",
            spec.role_name
        );
        // The members the SERVICE owns are derived: a task AWS hands back is running, and its
        // percentage is a share of elapsed time rather than anything the caller sent.
        assert_eq!(
            created.status, EXPORT_IMAGE_TASK_STATUS_ACTIVE,
            "case {case:?}: a task just started is {EXPORT_IMAGE_TASK_STATUS_ACTIVE}, got {:?}",
            created.status
        );
        let percent: u64 = created
            .progress
            .as_deref()
            .unwrap_or_default()
            .parse()
            .unwrap_or_else(|_| {
                panic!(
                    "case {case:?}: expected a percentage of progress, got {:?}",
                    created.progress
                )
            });
        assert!(
            percent <= MAX_ACTIVE_IMPORT_PROGRESS,
            "case {case:?}: expected progress at most {MAX_ACTIVE_IMPORT_PROGRESS}, got {percent}"
        );
        assert_eq!(
            created.status_message, None,
            "case {case:?}: a task running to plan reports no status message, got {:?}",
            created.status_message
        );
        expected.push((case, created, image_id));
    }

    assert_eq!(
        stored_count(&engine),
        table.len(),
        "every export must have stored a task of its own"
    );

    // A call that narrows nothing reports every task the account holds, each one exactly as the
    // create reported it: the describe reads the STORE, not the request.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the unnarrowed describe must succeed, got {status} with body {body}"
    );
    let found = reported("the unnarrowed report", &body);
    assert_eq!(
        found.len(),
        table.len(),
        "expected {} tasks in the report, got {found:?}",
        table.len()
    );
    for (case, created, _) in &expected {
        let reported_back = found
            .iter()
            .find(|task| task.export_image_task_id == created.export_image_task_id)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {} is not in the report {found:?}",
                    created.export_image_task_id
                )
            });
        // Everything but the progress, which has moved on with the clock, is the task the
        // create reported.
        assert_eq!(
            Reported {
                progress: created.progress.clone(),
                ..reported_back.clone()
            },
            *created,
            "case {case:?}: the describe must report the task the create stored"
        );
    }
}

/// RULE 2: the clock moves an export `active` -> `completed`, and a read reports the move
/// without a second call having made it happen.
#[tokio::test]
async fn the_clock_moves_an_export_from_active_to_completed() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_image(&engine, "clock-moves-the-export");
    let case = "an export left alone for its whole window";
    let task_id = export_ok(
        &router,
        case,
        &ExportSpec::new(&image_id, "VMDK", "vm-exports"),
    )
    .await;

    let (_, body) = describe(&router, &ids(&[task_id.as_str()])).await;
    assert_eq!(
        reported_one(case, &body).status,
        EXPORT_IMAGE_TASK_STATUS_ACTIVE,
        "case {case:?}: before its window has run out the task is \
         {EXPORT_IMAGE_TASK_STATUS_ACTIVE}, body {body}"
    );

    past_the_transition().await;

    // Nothing was called in between: the move is owed by the clock, so the FIRST read after
    // the window reports it.
    assert_eq!(
        stored_status(&engine, &task_id),
        EXPORT_IMAGE_TASK_STATUS_COMPLETED,
        "case {case:?}: the store itself must have moved the task on the clock"
    );
    let (_, body) = describe(&router, &ids(&[task_id.as_str()])).await;
    let completed = reported_one(case, &body);
    assert_eq!(
        completed.status, EXPORT_IMAGE_TASK_STATUS_COMPLETED,
        "case {case:?}: once its window has run out the task is \
         {EXPORT_IMAGE_TASK_STATUS_COMPLETED}, body {body}"
    );
    assert_eq!(
        completed.progress, None,
        "case {case:?}: a task that is no longer running reports no percentage, got {:?}",
        completed.progress
    );
}

/// RULE 2: `CancelExportTask` puts an image export task into `deleting` and the clock owes it
/// `deleted`; a task that is not active is refused with the status it is in.
#[tokio::test]
async fn cancelling_an_active_export_image_task_finishes_on_the_clock() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_image(&engine, "cancel-the-export");
    let case = "an export cancelled while it was still running";
    let task_id = export_ok(
        &router,
        case,
        &ExportSpec::new(&image_id, "VHD", "vm-exports"),
    )
    .await;

    let (status, body) = cancel(&router, &task_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: cancelling an active task must succeed, got {status} with body {body}"
    );
    let (_, body) = describe(&router, &ids(&[task_id.as_str()])).await;
    let cancelling = reported_one(case, &body);
    assert_eq!(
        cancelling.status, EXPORT_IMAGE_TASK_STATUS_DELETING,
        "case {case:?}: straight after the cancel the task is \
         {EXPORT_IMAGE_TASK_STATUS_DELETING}, body {body}"
    );
    assert_eq!(
        cancelling.progress, None,
        "case {case:?}: a task being taken down reports no percentage, got {:?}",
        cancelling.progress
    );

    past_the_transition().await;

    let (_, body) = describe(&router, &ids(&[task_id.as_str()])).await;
    assert_eq!(
        reported_one(case, &body).status,
        EXPORT_IMAGE_TASK_STATUS_DELETED,
        "case {case:?}: once the cancellation falls due the task is \
         {EXPORT_IMAGE_TASK_STATUS_DELETED}, body {body}"
    );

    // "Cancels an ACTIVE export task": one already being taken down is refused with the status
    // it is in rather than cancelled twice.
    let (status, body) = cancel(&router, &task_id).await;
    assert_eq!(
        status, 400,
        "case {case:?}: cancelling a settled task must be refused, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        INCORRECT_STATE,
        "case {case:?}: expected {INCORRECT_STATE}, got {body}"
    );
}

/// Store four export image tasks, one in each published status, and answer their ids in the
/// order [`EXPORT_IMAGE_TASK_STATUS_VALUES`] lists the statuses.
///
/// Each status is reached the way a caller reaches it — a create, a wait, a cancel — so no row
/// is written into the store behind the API.
async fn tasks_in_every_status(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
) -> Vec<String> {
    let completed_image = seed_image(engine, "filter-completed");
    let deleted_image = seed_image(engine, "filter-deleted");
    let completed = export_ok(
        router,
        "a task left to finish",
        &ExportSpec::new(&completed_image, "VMDK", "vm-exports"),
    )
    .await;
    let deleted = export_ok(
        router,
        "a task cancelled and left to settle",
        &ExportSpec::new(&deleted_image, "RAW", "vm-exports"),
    )
    .await;
    let (status, body) = cancel(router, &deleted).await;
    assert_eq!(
        status, 200,
        "the cancel that leads to {EXPORT_IMAGE_TASK_STATUS_DELETED} must succeed: {body}"
    );

    past_the_transition().await;

    let active_image = seed_image(engine, "filter-active");
    let deleting_image = seed_image(engine, "filter-deleting");
    let active = export_ok(
        router,
        "a task still running",
        &ExportSpec::new(&active_image, "VHD", "vm-exports"),
    )
    .await;
    let deleting = export_ok(
        router,
        "a task being taken down",
        &ExportSpec::new(&deleting_image, "VMDK", "vm-exports"),
    )
    .await;
    let (status, body) = cancel(router, &deleting).await;
    assert_eq!(
        status, 200,
        "the cancel that leads to {EXPORT_IMAGE_TASK_STATUS_DELETING} must succeed: {body}"
    );

    let tasks = vec![active, completed, deleting, deleted];
    for (status_value, task_id) in EXPORT_IMAGE_TASK_STATUS_VALUES.iter().zip(&tasks) {
        assert_eq!(
            &stored_status(engine, task_id),
            status_value,
            "the task {task_id} must be in the {status_value} status for the table below"
        );
    }
    tasks
}

/// One row of the filter table: the case, the `Filters` entries it sends, and the tasks it must
/// select.
type FilterRow<'a> = (&'a str, Vec<(&'a str, &'a [&'a str])>, Vec<&'a String>);

/// RULE 4: the `task-state` filter selects over the published status catalog with EC2's AND/OR
/// rule, and a name this operation does not document selects nothing rather than being refused.
#[tokio::test]
async fn the_task_state_filter_selects_over_the_published_status_catalog() {
    let (router, engine) = super::ec2_query::emulator();
    let tasks = tasks_in_every_status(&router, &engine).await;
    let [active, completed, deleting, deleted] = <[String; 4]>::try_from(tasks)
        .unwrap_or_else(|tasks| panic!("four tasks, one per published status, got {tasks:?}"));

    // The rows differ along WHICH status is asked for, HOW MANY values one filter carries (the
    // OR), and how many filters are given (the AND).
    let the_running_task: [&str; 1] = [active.as_str()];
    let rows: Vec<FilterRow<'_>> = vec![
        (
            "one value: the running task",
            vec![("task-state", &[EXPORT_IMAGE_TASK_STATUS_ACTIVE])],
            vec![&active],
        ),
        (
            "one value: the finished task",
            vec![("task-state", &[EXPORT_IMAGE_TASK_STATUS_COMPLETED])],
            vec![&completed],
        ),
        (
            "one value: the task being taken down",
            vec![("task-state", &[EXPORT_IMAGE_TASK_STATUS_DELETING])],
            vec![&deleting],
        ),
        (
            "one value: the task whose cancellation is done",
            vec![("task-state", &[EXPORT_IMAGE_TASK_STATUS_DELETED])],
            vec![&deleted],
        ),
        (
            "two values are joined with an OR",
            vec![(
                "task-state",
                &[
                    EXPORT_IMAGE_TASK_STATUS_ACTIVE,
                    EXPORT_IMAGE_TASK_STATUS_DELETED,
                ],
            )],
            vec![&active, &deleted],
        ),
        (
            "every published value selects every task",
            vec![("task-state", &EXPORT_IMAGE_TASK_STATUS_VALUES[..])],
            vec![&active, &completed, &deleting, &deleted],
        ),
        (
            "a wildcard value matches the statuses it spans",
            vec![("task-state", &["delet*"])],
            vec![&deleting, &deleted],
        ),
        (
            "two filters are joined with an AND, so disjoint values select nothing",
            vec![
                ("task-state", &[EXPORT_IMAGE_TASK_STATUS_ACTIVE]),
                ("task-state", &[EXPORT_IMAGE_TASK_STATUS_COMPLETED]),
            ],
            vec![],
        ),
        (
            "a status no task is in selects nothing",
            vec![("task-state", &["cancelled"])],
            vec![],
        ),
        (
            "a filter name this operation does not document selects nothing",
            vec![("export-image-task-id", &the_running_task)],
            vec![],
        ),
    ];

    for (case, entries, expected) in rows {
        let found = selected(&router, case, &filters(&entries)).await;
        let mut expected: Vec<String> = expected.into_iter().cloned().collect();
        expected.sort();
        assert_eq!(
            found, expected,
            "case {case:?}: expected the tasks {expected:?}, got {found:?}"
        );
    }
}

/// RULE 3: `ExportImageTaskIds` narrows the report, and an id the account holds no task for
/// selects no row rather than failing the call.
#[tokio::test]
async fn export_image_task_ids_narrow_the_report_and_an_unheld_id_selects_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let mut started: Vec<String> = Vec::new();
    for index in 0..3 {
        let image_id = seed_image(&engine, &format!("narrowing-{index}"));
        started.push(
            export_ok(
                &router,
                "a task to narrow the report to",
                &ExportSpec::new(&image_id, "VMDK", "vm-exports"),
            )
            .await,
        );
    }
    let held: Vec<&str> = started.iter().map(String::as_str).collect();

    // The rows differ along HOW MANY ids are named and whether each one is held: a well-formed
    // id of the right family that names no task, a short-form id, and something that is no
    // export task id at all all select no row — the describe reports the set it was asked
    // about, which is then empty.
    let unheld_long = format!("{EXPORT_IMAGE_TASK_ID_PREFIX}-0ffffffffffffffff");
    let unheld_short = format!("{EXPORT_IMAGE_TASK_ID_PREFIX}-0fffffff");
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("no ids at all reports every task", vec![], held.clone()),
        ("one id reports that task", vec![held[1]], vec![held[1]]),
        (
            "two ids report both",
            vec![held[0], held[2]],
            vec![held[0], held[2]],
        ),
        (
            "a held id beside an unheld one reports the held one",
            vec![held[0], unheld_long.as_str()],
            vec![held[0]],
        ),
        (
            "an unheld long-form id selects nothing",
            vec![unheld_long.as_str()],
            vec![],
        ),
        (
            "an unheld short-form id selects nothing",
            vec![unheld_short.as_str()],
            vec![],
        ),
        (
            "a value that is no export task id selects nothing",
            vec!["not-an-export-task-id"],
            vec![],
        ),
    ];

    for (case, named, expected) in rows {
        let found = selected(&router, case, &ids(&named)).await;
        let mut expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(
            found, expected,
            "case {case:?}: expected the tasks {expected:?}, got {found:?}"
        );
    }

    // More ids than the request may carry is refused rather than scanned.
    let too_many: Vec<String> = (0..=MAX_RESOURCE_IDS)
        .map(|index| format!("{EXPORT_IMAGE_TASK_ID_PREFIX}-{index:017x}"))
        .collect();
    let (status, body) = describe(
        &router,
        &ids(&too_many.iter().map(String::as_str).collect::<Vec<&str>>()),
    )
    .await;
    assert_eq!(
        status, 400,
        "more than {MAX_RESOURCE_IDS} ids must be refused, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "expected {INVALID_PARAMETER_VALUE} past the id cap, got {body}"
    );
}

/// RULE 5: `MaxResults` and `NextToken` page the report in `ExportImageTaskId` order, and a
/// token this service did not mint is refused.
#[tokio::test]
async fn max_results_and_next_token_page_the_report_in_id_order() {
    let (router, engine) = super::ec2_query::emulator();
    let mut started: Vec<String> = Vec::new();
    for index in 0..5 {
        let image_id = seed_image(&engine, &format!("paging-{index}"));
        started.push(
            export_ok(
                &router,
                "a task to page over",
                &ExportSpec::new(&image_id, "RAW", "vm-exports"),
            )
            .await,
        );
    }
    let mut every_id = started.clone();
    every_id.sort();

    // The rows differ along the PAGE SIZE: one task at a time, several, exactly the whole set,
    // and more than the account holds.
    for page_size in [1usize, 2, 3, 5, 7] {
        let case = format!("a walk in pages of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = &token {
                params.push(p("NextToken", token.clone()));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {pages} must succeed, got {status} with body {body}"
            );
            let page = reported_ids(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} tasks, got {page:?}"
            );
            walked.extend(page.clone());
            pages += 1;
            assert!(
                pages <= every_id.len() + 1,
                "case {case:?}: the walk must end, it has taken {pages} pages"
            );
            match super::ec2_query::member(&body, "nextToken") {
                // "This value is null when there are no more results to return": the last page
                // of a walk carries no token.
                None => break,
                Some(next) => token = Some(next),
            }
        }
        assert_eq!(
            walked, every_id,
            "case {case:?}: the walk must report every task exactly once, in ascending id \
             order"
        );
    }

    // "A token that indicates the next page of results": one this service did not mint names no
    // position, so it is refused rather than read as an id.
    for (case, token) in [
        ("a token that is not hex", "not-a-token"),
        ("a token whose checksum does not match", "6563322d313a30"),
    ] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the token to be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected {INVALID_PAGINATION_TOKEN}, got {body}"
        );
    }
}

/// RULE 6: a request EC2 will not accept is refused with EC2's own code, and nothing is stored.
#[tokio::test]
async fn a_request_ec2_will_not_accept_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_image(&engine, "refusals");
    let too_long = "d".repeat(MAX_DESCRIPTION_CHARS + 1);
    let past_the_longest_bucket = bucket_of_length(MAX_S3_BUCKET_NAME_CHARS + 1);
    let short_of_the_shortest_bucket = bucket_of_length(MIN_S3_BUCKET_NAME_CHARS - 1);

    // The rows differ along WHICH member is wrong and HOW: a required one left out, a published
    // bound overrun by one, a bucket name outside each of the Amazon S3 naming rules, and an
    // AMI id of each form the account cannot resolve.
    //
    // `DiskImageFormat` is not a row: it targets a modelled enum, so a value outside
    // `DiskImageFormat`'s members — the empty value an omitted member arrives as included — is
    // refused by the request boundary before this operation's handler runs, which makes the
    // handler's own check of it a backstop rather than something a wire request can reach.
    let rows: Vec<(&str, ExportSpec<'_>, &str)> = vec![
        (
            "no ImageId",
            ExportSpec::new("", "VMDK", "vm-exports"),
            MISSING_PARAMETER,
        ),
        (
            "no destination bucket",
            ExportSpec::new(&image_id, "VMDK", ""),
            MISSING_PARAMETER,
        ),
        (
            "a bucket name carrying characters the rules forbid",
            ExportSpec::new(&image_id, "VMDK", "VM-Exports"),
            INVALID_REQUEST,
        ),
        (
            "a bucket name one character past the longest the rules allow",
            ExportSpec::new(&image_id, "VMDK", &past_the_longest_bucket),
            INVALID_REQUEST,
        ),
        (
            "a bucket name one character short of the shortest the rules allow",
            ExportSpec::new(&image_id, "VMDK", &short_of_the_shortest_bucket),
            INVALID_REQUEST,
        ),
        (
            "a well-formed AMI id the account holds no image for",
            ExportSpec::new(UNKNOWN_IMAGE_ID, "VMDK", "vm-exports"),
            INVALID_AMI_ID_NOT_FOUND,
        ),
        (
            "a value that is not an AMI id at all",
            ExportSpec::new(MALFORMED_IMAGE_ID, "VMDK", "vm-exports"),
            INVALID_AMI_ID_MALFORMED,
        ),
        (
            "a description one character past its published maximum",
            ExportSpec {
                description: Some(&too_long),
                ..ExportSpec::new(&image_id, "VMDK", "vm-exports")
            },
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a tag specification naming a resource type this action does not tag",
            ExportSpec {
                tags: &[("owner", "platform")],
                tag_resource_type: "image",
                ..ExportSpec::new(&image_id, "VMDK", "vm-exports")
            },
            INVALID_PARAMETER_VALUE,
        ),
    ];

    for (case, spec, expected_code) in &rows {
        let (status, body) = api(&router, "ExportImage", &spec.params()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status} with body {body}"
        );
        assert_eq!(
            &error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused request must store no export image task"
        );
    }

    // The published bound itself is ACCEPTED: a description of exactly the maximum length is a
    // description EC2 takes, which is the other side of the row above.
    let at_the_bound = "d".repeat(MAX_DESCRIPTION_CHARS);
    let case = "a description of exactly its published maximum";
    let task_id = export_ok(
        &router,
        case,
        &ExportSpec {
            description: Some(&at_the_bound),
            ..ExportSpec::new(&image_id, "VMDK", "vm-exports")
        },
    )
    .await;
    let (_, body) = describe(&router, &ids(&[task_id.as_str()])).await;
    assert_eq!(
        reported_one(case, &body).description,
        at_the_bound,
        "case {case:?}: the description at the bound must round-trip, body {body}"
    );
}

/// RULE 6: "Token to enable idempotency for export image requests" — a replay reports the task
/// the first call started and starts no second export.
#[tokio::test]
async fn a_client_token_replay_reports_the_same_task_and_starts_no_second_export() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_image(&engine, "idempotent-export");
    let case = "the same request sent twice under one token";
    let spec = ExportSpec {
        client_token: Some("c0ffee-export-token"),
        description: Some("exported once"),
        ..ExportSpec::new(&image_id, "VMDK", "vm-exports")
    };

    let first = export_ok(&router, case, &spec).await;
    let second = export_ok(&router, case, &spec).await;
    assert_eq!(
        first, second,
        "case {case:?}: a replay must report the task the first call started"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "case {case:?}: a replay must store no second export image task"
    );

    // A DIFFERENT token is a different request, so it starts a task of its own.
    let other = export_ok(
        &router,
        case,
        &ExportSpec {
            client_token: Some("another-export-token"),
            ..spec.clone()
        },
    )
    .await;
    assert_ne!(
        other, first,
        "case {case:?}: a different token must start a task of its own"
    );
    assert_eq!(
        stored_count(&engine),
        2,
        "case {case:?}: the second token must have stored a second task"
    );
}

/// RULE 2 and 6: the report is a READ — two identical calls answer the same set, and neither
/// the statuses the store holds nor the number of tasks is changed by reading them.
#[tokio::test]
async fn describing_export_image_tasks_changes_nothing_a_later_caller_sees() {
    let (router, engine) = super::ec2_query::emulator();
    let tasks = tasks_in_every_status(&router, &engine).await;

    let before: Vec<String> = tasks
        .iter()
        .map(|task_id| stored_status(&engine, task_id))
        .collect();
    let count_before = stored_count(&engine);

    let first = selected(&router, "the first read", &[]).await;
    let second = selected(&router, "the second read", &[]).await;
    assert_eq!(
        first, second,
        "two identical calls must answer the same set, got {first:?} then {second:?}"
    );
    // Narrowing and filtering are reads too, so they are made here as well before the store is
    // compared.
    let _ = selected(
        &router,
        "a narrowed read",
        &ids(&[tasks[0].as_str(), tasks[3].as_str()]),
    )
    .await;
    let _ = selected(
        &router,
        "a filtered read",
        &filters(&[("task-state", &[EXPORT_IMAGE_TASK_STATUS_ACTIVE])]),
    )
    .await;

    let after: Vec<String> = tasks
        .iter()
        .map(|task_id| stored_status(&engine, task_id))
        .collect();
    assert_eq!(
        after, before,
        "reading the tasks must not move any of them, {before:?} became {after:?}"
    );
    assert_eq!(
        stored_count(&engine),
        count_before,
        "reading the tasks must not add or drop one"
    );
}
