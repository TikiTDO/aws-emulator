// SPDX-License-Identifier: CC0-1.0

//! What a submitted S3 Batch Operations job that needs NO confirmation settles
//! as, driven end to end over HTTP against Amazon S3 and S3 Control on one
//! shared engine.
//!
//! The rule under test: a created job for which no confirmation is required
//! needs no further caller input — `CreateJob` answers with its id and the job
//! is then advanced on its own, never entering `Suspended` (which
//! `JobDescriptor$SuspendedCause` documents as reached only by a job created
//! through the console, to await confirmation), to a terminal status a caller
//! polling `DescribeJob` observes, carrying the `TerminationDate` the model
//! documents as "a timestamp indicating when this job terminated" and a
//! `ProgressSummary` whose `TotalNumberOfTasks` accounts for every row of the
//! job's object list. Its `S3PutObjectTagging` action — which "passes every
//! object to the underlying `PutObjectTagging` API operation" — leaves each
//! listed object carrying EXACTLY the job's tag set, replacing whatever tag set
//! that object held, and leaves an object the list does not name with the tag
//! set it had.
//!
//! The rows span what the rule generalises over: whether the create names
//! `ConfirmationRequired` at all or names it false, how many objects the
//! manifest lists and how many the bucket holds beside them, how many tags the
//! job's action carries, whether the listed objects were untagged or already
//! carried another tag set, and which characters the tag keys and values use of
//! those `S3Tag$Key` and `S3Tag$Value` admit (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`,
//! from `smithy_query(query="shape", shape="S3Tag")`).
//!
//! Every tag read back here is one the job's own run applied, through Amazon
//! S3's own API, and every object is one the shared object store itself holds.

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    JOB_STATUS_COMPLETE, JOB_STATUS_SUSPENDED,
};

use crate::access_point_lifecycle::{xml_escape, REGION, XMLNS};
use crate::batch_job_execution::{csv_manifest, describe, poll_terminal, progress};
use crate::object_store::ObjectStore;

/// The IAM role every job below names — the modelled `IAMRoleArn` pattern
/// (`^arn:[^:]+:iam::\d{12}:role/`). This store holds no IAM record under it,
/// which is evidence about this store rather than about the role, so such a job
/// runs ([`crate::batch_job_role_trust`] pins what a role IAM DOES hold a trust
/// document for decides).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/S3BatchOperations";

/// One object the row's bucket holds: its key and the tag set it carries before
/// the job runs.
type Object = (&'static str, &'static [(&'static str, &'static str)]);

/// One job the rule is asserted over.
struct Row {
    case: &'static str,
    /// What the create says about `ConfirmationRequired`: `Some(false)` for a
    /// create that names it, `None` for one that leaves the member out.
    confirmation_required: Option<bool>,
    /// Every object written into the row's bucket, with the tag set it starts
    /// with.
    objects: &'static [Object],
    /// The keys the job's manifest lists, in manifest order; the rest of
    /// `objects` must be left alone.
    listed: &'static [&'static str],
    /// The tag set the job's `S3PutObjectTagging` action carries.
    tag_set: &'static [(&'static str, &'static str)],
}

/// The job table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "confirmation not named, one untagged object of two, one tag",
            confirmation_required: None,
            objects: &[("obj/a.txt", &[]), ("obj/kept.txt", &[("keep", "me")])],
            listed: &["obj/a.txt"],
            tag_set: &[("review", "done")],
        },
        Row {
            case: "confirmation named false, two of three objects, two tags over prior tags",
            confirmation_required: Some(false),
            objects: &[
                (
                    "data/one.csv",
                    &[("stage", "raw"), ("owner", "finance"), ("drop", "this")],
                ),
                ("data/two.csv", &[]),
                ("data/three.csv", &[("stage", "raw")]),
            ],
            listed: &["data/one.csv", "data/two.csv"],
            tag_set: &[("stage", "archived"), ("retention", "30d")],
        },
        Row {
            case: "confirmation named false, every object listed, tags using the key pattern's \
                   punctuation",
            confirmation_required: Some(false),
            objects: &[
                ("logs/alpha", &[("cost:center/eu", "old")]),
                ("logs/beta", &[]),
                ("logs/gamma", &[("unrelated", "tag")]),
            ],
            listed: &["logs/alpha", "logs/beta", "logs/gamma"],
            tag_set: &[
                ("cost:center/eu", "2026-Q1+final"),
                ("path.part", "a/b=c@d"),
                ("plain", "value"),
            ],
        },
    ]
}

/// The row's tag set as the `<TagSet>` body of an `S3PutObjectTagging` action.
fn tag_set_xml(tags: &[(&str, &str)]) -> String {
    let members: String = tags
        .iter()
        .map(|(key, value)| {
            format!(
                "<member><Key>{}</Key><Value>{}</Value></member>",
                xml_escape(key),
                xml_escape(value)
            )
        })
        .collect();
    format!("<TagSet>{members}</TagSet>")
}

/// `(key, value)` pairs, sorted, so a comparison is about which tags an object
/// carries rather than the order one answer happened to list them in.
fn sorted(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    pairs.sort();
    pairs
}

/// RULE: a job needing no confirmation is advanced with no further caller
/// input, never through `Suspended`, to a terminal status carrying a
/// `TerminationDate` and a `ProgressSummary` accounting for every row of its
/// object list — and its tagging action leaves every listed object carrying
/// exactly the job's tag set while every other object keeps the one it had.
#[tokio::test]
async fn a_job_needing_no_confirmation_settles_and_tags_the_objects_its_manifest_lists() {
    for (row_at, row) in rows().iter().enumerate() {
        let store = ObjectStore::start();
        let bucket = format!("batch-unconfirmed-{row_at}");
        store.create_bucket(row.case, REGION, &bucket).await;
        for (key, tags) in row.objects {
            store
                .put_object(row.case, REGION, &bucket, key, b"to-be-tagged")
                .await;
            if !tags.is_empty() {
                store
                    .put_object_tags(row.case, REGION, &bucket, key, tags)
                    .await;
            }
        }
        let manifest_body: String = row
            .listed
            .iter()
            .map(|key| format!("{bucket},{key}\n"))
            .collect();
        let e_tag = store
            .put_object(
                row.case,
                REGION,
                &bucket,
                "manifest.csv",
                manifest_body.as_bytes(),
            )
            .await;

        let confirmation = match row.confirmation_required {
            Some(required) => {
                format!("<ConfirmationRequired>{required}</ConfirmationRequired>")
            }
            None => String::new(),
        };
        let body = format!(
            "<CreateJobRequest xmlns=\"{XMLNS}\">\
             <ClientRequestToken>unconfirmed-{row_at}</ClientRequestToken>{confirmation}\
             <Operation><S3PutObjectTagging>{}</S3PutObjectTagging></Operation>\
             <Report><Enabled>false</Enabled></Report>{}\
             <Priority>10</Priority><RoleArn>{ROLE_ARN}</RoleArn></CreateJobRequest>",
            tag_set_xml(row.tag_set),
            csv_manifest(&bucket, "manifest.csv", &e_tag)
        );
        let created = store
            .s3control(REGION, "POST", "/v20180820/jobs", &body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateJob to answer 200, got {} with body {}",
            row.case, created.status, created.body
        );
        let job_id = created.text("JobId").unwrap_or_else(|| {
            panic!(
                "case {:?}: CreateJob answers a JobId, got body {}",
                row.case, created.body
            )
        });

        // The job is not awaiting anything: whatever status it is in when the
        // create answers, it is not the one a job created through the console
        // enters to await a confirmation.
        let first = describe(&store, &job_id).await;
        assert_ne!(
            first.text("Status").as_deref(),
            Some(JOB_STATUS_SUSPENDED),
            "case {:?}: expected a job needing no confirmation never to await one, got body {}",
            row.case,
            first.body
        );
        assert!(
            first.text("SuspendedCause").is_none() && first.text("SuspendedDate").is_none(),
            "case {:?}: expected no suspension on a job needing no confirmation, got body {}",
            row.case,
            first.body
        );

        // Nothing but DescribeJob is called from here: the job settles on its
        // own.
        let settled = poll_terminal(&store, row.case, &job_id).await;
        assert_eq!(
            settled.text("Status").as_deref(),
            Some(JOB_STATUS_COMPLETE),
            "case {:?}: expected the job's tasks to have run, leaving it complete, got body {}",
            row.case,
            settled.body
        );
        assert!(
            settled
                .text("TerminationDate")
                .is_some_and(|date| !date.is_empty()),
            "case {:?}: expected a terminated job to report when it terminated, got body {}",
            row.case,
            settled.body
        );
        let listed = i64::try_from(row.listed.len()).expect("the row count fits");
        assert_eq!(
            progress(row.case, &settled),
            (listed, listed, 0),
            "case {:?}: expected every manifest row accounted for as a task that succeeded, got \
             body {}",
            row.case,
            settled.body
        );

        // Every listed object carries exactly the job's tag set, whatever tag
        // set it held before; everything else is left as it was.
        for (key, before) in row.objects {
            let expected = if row.listed.contains(key) {
                sorted(row.tag_set)
            } else {
                sorted(before)
            };
            let mut found = store.object_tags(REGION, &bucket, key).await;
            found.sort();
            assert_eq!(
                found, expected,
                "case {:?}: expected {key} to carry {expected:?}, got {found:?}",
                row.case
            );
        }
    }
}
