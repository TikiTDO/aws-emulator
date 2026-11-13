// SPDX-License-Identifier: CC0-1.0

//! What PUTS the data segments on a session dataset, driven end to end over HTTP
//! through the same `MultiServiceRouter` the emulator boots with — so these
//! assertions are about the wire behaviour a real client sees.
//!
//! The rule these suites pin: a session dataset "contains data segments of time
//! series data" and an import names it as "the session dataset to ingest data
//! into", so an accepted bulk import is what records that dataset's segments —
//! one per file whose `alias` names a data stream this store holds, covering that
//! stream's stored data from the instant the file states (its first stored sample
//! when the file states none) — and the dataset then lists those segments, takes
//! its next version, and `BatchDeleteDatasetDataSegments` removes one of them
//! together with the samples inside its own span. A file naming no stream this
//! store holds, or an instant past every sample that stream holds, brings in
//! nothing, and a span overlapping a segment already held records nothing, so a
//! repeated import adds no second segment.
//!
//! No SAMPLE is read from Amazon S3: the data a segment covers is the data the
//! store really holds for the stream the file names, ingested through
//! `BatchPutAssetPropertyValue`. (An `mp4` file carries media rather than
//! samples, and what one brings in is video — `tests/all/capture_import.rs`.)
//!
//! The rows differ along what the rule generalises over: where the file's
//! `startTime` falls relative to the stream's samples (at the first, between two,
//! at the last, past them all) and whether it is stated at all, how many files one
//! job names, which kind of stream a file's alias names (one no asset property
//! owns and one a property does), an alias no stream answers to and a file naming
//! none, and two files whose spans coincide or overlap.
//!
//! The HTTP harness (`call`, `router_with_engine`, `create_model`, `create_asset`,
//! `property_id`, `is_modelled_id`) is the asset suite's and the dataset, ingest,
//! clock and history helpers are the data-segment suite's; both are reused rather
//! than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{
    call, create_asset, create_model, is_modelled_id, property_id, router_with_engine,
};
use crate::dataset_data_segments::{
    at, create_dataset, history_seconds, ingest, relative, time_series_id,
};

/// The workspace the session dataset of every row lives in.
const WORKSPACE: &str = "press-floor";

/// The alias of the stream no asset property owns.
const LOOSE_ALIAS: &str = "/plant/import/loose";

/// A second such stream, so a row can name one file per stream.
const SECOND_ALIAS: &str = "/plant/import/second";

/// The alias of the stream an asset property owns, so a row can prove the rule
/// reaches both identifying forms a stored stream has.
const BOUND_ALIAS: &str = "/plant/import/bound";

/// An alias no stream of this store answers to.
const UNKNOWN_ALIAS: &str = "/plant/import/nothing-here";

/// The table seconds [`LOOSE_ALIAS`] holds samples at.
const LOOSE_SAMPLES: &[i64] = &[0, 5, 10, 15, 100];

/// The same for [`SECOND_ALIAS`].
const SECOND_SAMPLES: &[i64] = &[20, 25];

/// The same for [`BOUND_ALIAS`].
const BOUND_SAMPLES: &[i64] = &[30, 35];

/// The `jobRoleArn` every create names: an ARN of the form the modelled `ARN`
/// pattern admits (`^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`).
const JOB_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SiteWiseBulkImport";

/// The `PropertyDataType` every stream of this fixture carries: each sample is a
/// `doubleValue`, and the asset property the bound stream belongs to is declared
/// `DOUBLE`.
const DATA_TYPE: &str = "DOUBLE";

/// Which stream a file's `alias` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    /// [`LOOSE_ALIAS`]: a stream no asset property owns.
    Loose,
    /// [`SECOND_ALIAS`]: a second stream of that kind.
    Second,
    /// [`BOUND_ALIAS`]: the stream an asset property owns.
    Bound,
    /// [`UNKNOWN_ALIAS`]: an alias no stream answers to.
    Unknown,
    /// A file that names no alias at all.
    Nameless,
}

/// The ids and aliases the fixture's streams were stored under.
#[derive(Debug, Clone)]
struct Streams {
    loose: String,
    second: String,
    bound: String,
}

impl Streams {
    /// The `TimeSeriesId` `stream` names, and the empty string for the two rows
    /// that name no stored stream.
    fn id(&self, stream: Stream) -> &str {
        match stream {
            Stream::Loose => &self.loose,
            Stream::Second => &self.second,
            Stream::Bound => &self.bound,
            Stream::Unknown | Stream::Nameless => "",
        }
    }
}

/// The `propertyAlias` `stream` answers to, if any.
fn alias_of(stream: Stream) -> Option<&'static str> {
    match stream {
        Stream::Loose => Some(LOOSE_ALIAS),
        Stream::Second => Some(SECOND_ALIAS),
        Stream::Bound => Some(BOUND_ALIAS),
        Stream::Unknown => Some(UNKNOWN_ALIAS),
        Stream::Nameless => None,
    }
}

/// One data segment as these suites compare them: the stream it covers, its
/// half-open span in table seconds plus nanosecond offsets, the alias and the
/// data type of that stream.
type Segment = (String, (i64, i32), (i64, i32), String, String);

/// One segment a row expects, as the table writes it: the stream it covers and
/// the half-open span it covers of it, in table seconds plus nanosecond offsets.
type Span = (Stream, (i64, i32), (i64, i32));

/// One row of the import table.
struct Case {
    label: &'static str,
    /// The `files` the job names: the stream each one's `alias` names, and the
    /// table second its `startTime` states, when it states one.
    files: &'static [(Stream, Option<i64>)],
    /// The segments the dataset must hold afterwards: the stream and the span, in
    /// table seconds plus nanosecond offsets.
    segments: &'static [Span],
}

/// The rows. Each stream's span ends ONE nanosecond past its last stored sample,
/// because a segment's span is half-open and has to cover that sample.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one-file-starting-at-the-streams-first-sample",
            files: &[(Stream::Loose, Some(0))],
            segments: &[(Stream::Loose, (0, 0), (100, 1))],
        },
        Case {
            label: "one-file-starting-between-two-samples",
            files: &[(Stream::Loose, Some(7))],
            segments: &[(Stream::Loose, (7, 0), (100, 1))],
        },
        Case {
            label: "one-file-starting-at-the-streams-last-sample",
            files: &[(Stream::Loose, Some(100))],
            segments: &[(Stream::Loose, (100, 0), (100, 1))],
        },
        Case {
            label: "one-file-stating-no-start-at-all",
            files: &[(Stream::Loose, None)],
            segments: &[(Stream::Loose, (0, 0), (100, 1))],
        },
        Case {
            label: "one-file-starting-past-every-sample-the-stream-holds",
            files: &[(Stream::Loose, Some(200))],
            segments: &[],
        },
        Case {
            label: "one-file-naming-the-stream-an-asset-property-owns",
            files: &[(Stream::Bound, None)],
            segments: &[(Stream::Bound, (30, 0), (35, 1))],
        },
        Case {
            label: "one-file-per-stream-in-one-job",
            files: &[
                (Stream::Loose, Some(0)),
                (Stream::Second, Some(20)),
                (Stream::Bound, Some(30)),
            ],
            segments: &[
                (Stream::Loose, (0, 0), (100, 1)),
                (Stream::Second, (20, 0), (25, 1)),
                (Stream::Bound, (30, 0), (35, 1)),
            ],
        },
        Case {
            label: "one-file-naming-no-alias",
            files: &[(Stream::Nameless, Some(0))],
            segments: &[],
        },
        Case {
            label: "one-file-whose-alias-names-no-stored-stream",
            files: &[(Stream::Unknown, Some(0))],
            segments: &[],
        },
        Case {
            label: "two-files-of-one-stream-at-the-same-start",
            files: &[(Stream::Loose, Some(0)), (Stream::Loose, Some(0))],
            segments: &[(Stream::Loose, (0, 0), (100, 1))],
        },
        Case {
            label: "two-files-of-one-stream-whose-spans-overlap",
            files: &[(Stream::Loose, Some(0)), (Stream::Loose, Some(10))],
            segments: &[(Stream::Loose, (0, 0), (100, 1))],
        },
        Case {
            label: "one-file-with-data-beside-one-with-none",
            files: &[(Stream::Loose, Some(5)), (Stream::Second, Some(900))],
            segments: &[(Stream::Loose, (5, 0), (100, 1))],
        },
    ]
}

/// A router over a fresh engine carrying a session dataset in [`WORKSPACE`], the
/// two streams no asset property owns, and the stream an asset property owns.
async fn fixture() -> (MultiServiceRouter, String, Streams) {
    let (router, _engine) = router_with_engine();
    let dataset = create_dataset(&router, "Import session", Some("SESSION"), Some(WORKSPACE)).await;

    ingest(&router, LOOSE_ALIAS, LOOSE_SAMPLES).await;
    ingest(&router, SECOND_ALIAS, SECOND_SAMPLES).await;
    ingest(&router, BOUND_ALIAS, BOUND_SAMPLES).await;

    let model = create_model(
        &router,
        "ImportPress",
        json!([{ "name": "Force", "dataType": DATA_TYPE, "type": { "measurement": {} } }]),
    )
    .await;
    let asset = create_asset(&router, "ImportPress1", &model).await;
    let (status, described) = call(&router, "GET", &format!("/assets/{asset}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {described}");
    let property = property_id(&described["assetProperties"], "Force");

    // The bound stream: the alias-addressed stream becomes the asset property's,
    // so the same alias now names a stream the pair of ids identifies.
    let (status, bound) = call(
        &router,
        "POST",
        &format!(
            "/timeseries/associate?alias={BOUND_ALIAS}&assetId={asset}&propertyId={property}"
        ),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "associate {BOUND_ALIAS}: got {status} {bound}");

    let streams = Streams {
        loose: time_series_id(&router, LOOSE_ALIAS).await,
        second: time_series_id(&router, SECOND_ALIAS).await,
        bound: time_series_id(&router, BOUND_ALIAS).await,
    };
    assert!(
        is_modelled_id(&streams.loose) && is_modelled_id(&streams.second),
        "a stream no property owns carries a modelled ID, got {streams:?}"
    );
    assert_eq!(
        streams.bound,
        format!("{asset}.{property}"),
        "a stream an asset property owns is named by the pair that owns it, got {streams:?}"
    );
    (router, dataset, streams)
}

/// One `File`: the bucket and key it is in, plus the alias and the start time the
/// row states.
fn file(stream: Stream, start: Option<i64>) -> Value {
    let mut value = json!({ "bucket": "sitewise-import", "key": "press/2026/01.csv" });
    let object = value.as_object_mut().expect("an object file");
    if let Some(alias) = alias_of(stream) {
        object.insert("alias".to_string(), json!(alias));
    }
    if let Some(start) = start {
        object.insert(
            "startTime".to_string(),
            json!({ "timeInSeconds": at(start), "offsetInNanos": 0 }),
        );
    }
    value
}

/// `CreateBulkImportJob` ingesting `files` into the session dataset `dataset` of
/// [`WORKSPACE`].
async fn import(
    router: &MultiServiceRouter,
    dataset: &str,
    files: &[(Stream, Option<i64>)],
) -> (u16, Value) {
    let files: Vec<Value> = files
        .iter()
        .map(|(stream, start)| file(*stream, *start))
        .collect();
    call(
        router,
        "POST",
        "/jobs",
        Some(json!({
            "jobName": "Press import",
            "jobRoleArn": JOB_ROLE_ARN,
            "files": files,
            "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
            "workspaceName": WORKSPACE,
            "datasetId": dataset,
        })),
    )
    .await
}

/// The segments `ListDatasetDataSegments` reports for `dataset`, each as the
/// [`Segment`] tuple a row compares, and the `sourceDatasetId` each one names.
async fn listed(router: &MultiServiceRouter, dataset: &str) -> (Vec<Segment>, Vec<String>) {
    let (status, body) = call(
        router,
        "GET",
        &format!("/datasets/{dataset}/data-segments?workspaceName={WORKSPACE}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListDatasetDataSegments: got {status} {body}");
    let reported = body["dataSegments"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a dataSegments list, got {body}"))
        .clone();
    let segments = reported
        .iter()
        .map(|segment| {
            (
                segment["timeSeriesId"].as_str().unwrap_or("").to_string(),
                bound_of(segment, "startTimestamp"),
                bound_of(segment, "endTimestamp"),
                segment["alias"].as_str().unwrap_or("").to_string(),
                segment["dataType"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let sources = reported
        .iter()
        .map(|segment| segment["sourceDatasetId"].as_str().unwrap_or("").to_string())
        .collect();
    (segments, sources)
}

/// One `TimeInNanos` member of a reported segment, as the table second and
/// nanosecond offset it was written from.
fn bound_of(segment: &Value, member: &str) -> (i64, i32) {
    (
        segment[member]["timeInSeconds"]
            .as_i64()
            .map(relative)
            .unwrap_or_else(|| panic!("a segment bound carries timeInSeconds, got {segment}")),
        segment[member]["offsetInNanos"].as_i64().unwrap_or(0) as i32,
    )
}

/// The `datasetVersion` `DescribeDataset` reports for `dataset`.
async fn dataset_version(router: &MultiServiceRouter, dataset: &str) -> String {
    let (status, body) = call(
        router,
        "GET",
        &format!("/datasets/{dataset}?workspaceName={WORKSPACE}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DescribeDataset: got {status} {body}");
    body["datasetVersion"].as_str().unwrap_or("").to_string()
}

/// The rows' expected segments, as the [`Segment`] tuples a listing is compared
/// against — in the total order the listing reports them in (the stream, then the
/// span).
fn expected(streams: &Streams, rows: &[Span]) -> Vec<Segment> {
    let mut expected: Vec<Segment> = rows
        .iter()
        .map(|(stream, start, end)| {
            (
                streams.id(*stream).to_string(),
                *start,
                *end,
                alias_of(*stream).unwrap_or_default().to_string(),
                DATA_TYPE.to_string(),
            )
        })
        .collect();
    expected.sort_by(|left, right| {
        (&left.0, left.1, left.2).cmp(&(&right.0, right.1, right.2))
    });
    expected
}

#[tokio::test]
async fn an_import_into_a_session_dataset_records_the_segments_its_files_bring_in() {
    for case in cases() {
        let Case {
            label,
            files,
            segments,
        } = case;
        let (router, dataset, streams) = fixture().await;

        let (status, started) = import(&router, &dataset, files).await;
        assert_eq!(
            status, 202,
            "case {label:?}: CreateBulkImportJob expected 202, got {status} {started}"
        );

        let (reported, sources) = listed(&router, &dataset).await;
        assert_eq!(
            reported,
            expected(&streams, segments),
            "case {label:?}: unexpected data segments on the session dataset"
        );
        assert!(
            sources.iter().all(|source| *source == dataset),
            "case {label:?}: a session dataset's own data came from the dataset that \
             holds it, got {sources:?}"
        );

        // "A dataset's version counts the writes it has accepted": the created
        // dataset stands at 1, and an import that recorded at least one segment
        // takes it to its next version.
        let version = if segments.is_empty() { "1" } else { "2" };
        let reported_version = dataset_version(&router, &dataset).await;
        assert_eq!(
            reported_version, version,
            "case {label:?}: expected datasetVersion {version}, got {reported_version}"
        );
    }
}

/// One row of the delete table: an import, then the deletion of the segment it
/// recorded.
struct DeleteCase {
    label: &'static str,
    /// The table second the imported file states as its start.
    start: Option<i64>,
    /// The span the deletion entry addresses, which is the span the import
    /// recorded.
    span: ((i64, i32), (i64, i32)),
    /// The table seconds of [`LOOSE_ALIAS`] that survive the deletion.
    remaining: &'static [i64],
}

/// The rows: a segment covering the stream's whole data, one starting between two
/// samples, and one covering only its last sample.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "the-segment-of-the-whole-stream",
            start: Some(0),
            span: ((0, 0), (100, 1)),
            remaining: &[],
        },
        DeleteCase {
            label: "a-segment-starting-between-two-samples",
            start: Some(7),
            span: ((7, 0), (100, 1)),
            remaining: &[0, 5],
        },
        DeleteCase {
            label: "a-segment-covering-the-last-sample-alone",
            start: Some(100),
            span: ((100, 0), (100, 1)),
            remaining: &[0, 5, 10, 15],
        },
    ]
}

#[tokio::test]
async fn a_deletion_removes_an_imported_segment_and_the_data_its_span_covers() {
    for case in delete_cases() {
        let DeleteCase {
            label,
            start,
            span,
            remaining,
        } = case;
        let (router, dataset, streams) = fixture().await;

        let (status, started) = import(&router, &dataset, &[(Stream::Loose, start)]).await;
        assert_eq!(
            status, 202,
            "case {label:?}: CreateBulkImportJob expected 202, got {status} {started}"
        );
        let (reported, _) = listed(&router, &dataset).await;
        assert_eq!(
            reported.len(),
            1,
            "case {label:?}: the import must record exactly one segment, got {reported:?}"
        );

        let (status, deleted) = call(
            &router,
            "POST",
            &format!("/datasets/{dataset}/data-segments/batch-delete"),
            Some(json!({
                "workspaceName": WORKSPACE,
                "deleteDataSegmentEntries": [{
                    "timeSeriesId": streams.id(Stream::Loose),
                    "startTimestamp": {
                        "timeInSeconds": at(span.0 .0),
                        "offsetInNanos": span.0 .1,
                    },
                    "endTimestamp": {
                        "timeInSeconds": at(span.1 .0),
                        "offsetInNanos": span.1 .1,
                    },
                }],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: BatchDeleteDatasetDataSegments expected 200, got {status} {deleted}"
        );
        assert_eq!(
            deleted["errors"].as_array().map(Vec::len),
            Some(0),
            "case {label:?}: the entry addressed the segment the import recorded, so \
             nothing may fail, got {deleted}"
        );
        assert_eq!(
            deleted["datasetVersion"].as_str(),
            Some("3"),
            "case {label:?}: the import took the dataset to 2 and the accepted deletion \
             to 3, got {deleted}"
        );

        // The segment is gone from the dataset, and the samples inside its own
        // span are gone from the stream it covered — while every sample outside
        // it, and the other streams' data, stay.
        let (reported, _) = listed(&router, &dataset).await;
        assert!(
            reported.is_empty(),
            "case {label:?}: the dataset must hold no segment, got {reported:?}"
        );
        let history = history_seconds(&router, LOOSE_ALIAS).await;
        assert_eq!(
            history, remaining,
            "case {label:?}: expected the samples {remaining:?} to survive, got {history:?}"
        );
        for (alias, samples) in [(SECOND_ALIAS, SECOND_SAMPLES), (BOUND_ALIAS, BOUND_SAMPLES)] {
            let untouched = history_seconds(&router, alias).await;
            assert_eq!(
                untouched, samples,
                "case {label:?}: the stream {alias} must keep every sample, got {untouched:?}"
            );
        }
    }
}
