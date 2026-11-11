// SPDX-License-Identifier: CC0-1.0

//! What an accepted import task READS: the data its `source` names, taken out of this
//! account's own object store and turned into the graph elements the load writes.
//!
//! An import is a real run here, not a promise: the location the request named is
//! listed and fetched through the emulator's OWN Amazon S3 over the in-process
//! callback, the objects are read in the requested `Format`
//! ([`super::import_formats`]), and what the load writes into the target graph is
//! exactly what those objects held. So every number `ImportTaskDetails` reports is a
//! measurement of that run, and a task reports `SUCCEEDED` only for data the graph
//! really holds afterwards.
//!
//! The read happens in the handler that accepts the task — the one place with a
//! request context to reach another service through — and the WRITE into the graph is
//! owed by the clock, so a caller that polls the task sees the load land at the
//! instant the task completes. Reading the source is not a state change, so it costs
//! an accepted request nothing to take it before the store is consulted.

use super::*;

/// The most objects one import reads out of its source location.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the sibling of
/// [`MAX_NODES_PER_GRAPH`]: the data a load reads is buffered in this process, so
/// without a ceiling one request could ask the emulator to hold an unbounded amount of
/// it. Set well above any realistic use of a local emulator.
pub const MAX_IMPORT_SOURCE_OBJECTS: usize = 64;

/// The most bytes one object of an import's source may hold.
pub const MAX_IMPORT_OBJECT_BYTES: usize = 4 * 1024 * 1024;

/// The most bytes one import reads out of its source location in total.
pub const MAX_IMPORT_SOURCE_BYTES: usize = 16 * 1024 * 1024;

/// The `statusReason` of a run whose `source` names no location this service reads.
///
/// "This can be an Amazon S3 path, or can point to a Neptune database endpoint or
/// snapshot" (`smithy_query(query="doc",shape="StartImportTaskInput",member="source")`)
/// — and a Neptune database is imported THROUGH Amazon S3, by way of the
/// `importOptions` the request supplies for it ([`neptune_export_path`]), so a source
/// that is neither is a location the load has nothing to read from.
pub const SOURCE_NOT_AN_S3_LOCATION_REASON: &str =
    "The import task's source is not an Amazon S3 location, and the request named no Neptune \
     import options carrying one, so the task had nothing to read.";

/// The `statusReason` of a run whose source location could not be read at all.
pub const SOURCE_UNREADABLE_REASON: &str =
    "The import task could not read its source location in Amazon S3.";

/// The `statusReason` of a run whose source location holds nothing.
pub const SOURCE_HOLDS_NOTHING_REASON: &str =
    "The import task's source location holds no object to import.";

/// Whether an import halts on the first error it meets when the request names no
/// `failOnError`.
///
/// From the documentation of the import options, which both minting operations repeat:
/// the import halts "if set to true, the default, or if omitted"
/// (`smithy_query(query="doc",shape="GetImportTaskOutput",member="importOptions")`).
/// Stated once here, beside the load the flag decides the fate of, so the two
/// operations that accept it cannot drift into different defaults.
pub const DEFAULT_FAIL_ON_ERROR: bool = true;

/// The elements one load writes into the graph it loads: the same node and edge records
/// the graph's data holds, so delivering a load is storing what it read.
#[derive(Debug, Clone, Default)]
pub struct ImportedData {
    /// Every node the load read, keyed by its node id.
    pub nodes: BTreeMap<String, GraphNode>,
    /// Every edge the load read, keyed by its edge id.
    pub edges: BTreeMap<String, GraphEdge>,
}

/// What reading an import task's source established: the elements to write, the counts
/// over the run, and the outcome the run ends in.
///
/// Carried on the stored task ([`ImportTask::load`]) because every one of these is a
/// measurement of ONE run: the task reports them, the graph takes the data, and
/// nothing recomputes either per read.
#[derive(Debug, Clone)]
pub struct ImportLoad {
    /// The elements the load writes when the clock makes its write due. Empty for a
    /// run that ends `FAILED`: a failed load is rolled back rather than left
    /// half-loaded.
    pub data: ImportedData,
    /// How many statements the load writes: what `ImportTaskDetails$statementCount`
    /// reports once the write has landed.
    pub statement_count: i64,
    /// How many distinct values the load interns: what
    /// `ImportTaskDetails$dictionaryEntryCount` reports once the write has landed.
    pub dictionary_entry_count: i64,
    /// How many rows, triples or objects the load could not read: what
    /// `ImportTaskDetails$errorCount` reports once the run is over.
    pub error_count: i32,
    /// What the first of those was: what `ImportTaskDetails$errorDetails` reports.
    pub error_details: Option<String>,
    /// Whether the load got as far as READING the source. A run that did not has no
    /// data to analyse or import, so it stops without reporting either stage.
    pub source_read: bool,
    /// Why the run ends `FAILED`, when it does. `None` for a run that ends
    /// `SUCCEEDED`: a load that did its work has nothing to explain.
    pub failure_reason: Option<String>,
}

impl ImportLoad {
    /// The load of a run that never read its source, and why.
    pub fn unread(reason: &str) -> Self {
        Self {
            data: ImportedData::default(),
            statement_count: 0,
            dictionary_entry_count: 0,
            error_count: 0,
            error_details: None,
            source_read: false,
            failure_reason: Some(reason.to_string()),
        }
    }
}

/// Read the data an import task's `source` names out of this emulator's own Amazon S3,
/// and report what that established.
///
/// Answers an [`ImportLoad`] rather than an error channel: reading the source is the
/// TASK's own work, so what it could not do is the task's own terminal status and
/// `statusReason`, never a refusal of the request that accepted it.
pub async fn read_import_source(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source: &str,
    format: Option<&str>,
    import_options: Option<&ImportOptions>,
    fail_on_error: bool,
) -> ImportLoad {
    // A Neptune database source is imported THROUGH the S3 export path the request's
    // `importOptions` name, so that is the location read for it.
    let location = neptune_export_path(import_options).unwrap_or(source);
    let Some((bucket, prefix)) = split_s3_uri(location) else {
        return ImportLoad::unread(SOURCE_NOT_AN_S3_LOCATION_REASON);
    };
    let client = ::service_common::s3_client::s3_client(ctx.callback(), region);
    let objects = match source_objects(&client, &bucket, &prefix).await {
        Ok(objects) => objects,
        Err(reason) => return ImportLoad::unread(&reason),
    };
    if objects.is_empty() {
        return ImportLoad::unread(SOURCE_HOLDS_NOTHING_REASON);
    }

    let read_as = ImportFormat::of(format);
    let mut builder = LoadBuilder::new(fail_on_error);
    let mut remaining = MAX_IMPORT_SOURCE_BYTES;
    for (key, listed) in &objects {
        if builder.has_failed() {
            break;
        }
        // The LISTED size decides whether the object is read at all, so the ceilings
        // bound what this process holds rather than being checked once it already holds
        // it.
        if *listed > MAX_IMPORT_OBJECT_BYTES || *listed > remaining {
            builder.fail(source_too_large_reason());
            break;
        }
        let fetched = client.get_object().bucket(&bucket).key(key).send().await;
        let Ok(object) = fetched else {
            builder.note_error("A source object could not be read.".to_string());
            continue;
        };
        let Ok(body) = object.body.collect().await else {
            builder.note_error("A source object could not be read.".to_string());
            continue;
        };
        let bytes = body.into_bytes();
        // The backstop for an object whose fetched bytes disagree with its listing.
        if bytes.len() > MAX_IMPORT_OBJECT_BYTES || bytes.len() > remaining {
            builder.fail(source_too_large_reason());
            break;
        }
        remaining -= bytes.len();
        builder.begin_document();
        // Decoding the object is CPU-bound and sized by what it holds, so it runs on a
        // BLOCKING task rather than on a runtime worker a large source would starve:
        // awaiting the join is a real await point, so a cancellation, a timeout and the
        // health endpoint keep working while one object decodes.
        let decoded = ::tokio::task::spawn_blocking(move || {
            read_as.read(&mut builder, &bytes);
            builder
        })
        .await;
        match decoded {
            Ok(carried) => builder = carried,
            // The decode task did not come back, so the load it was reading has no
            // elements to report and no counts that describe a run.
            Err(_) => return ImportLoad::unread(SOURCE_UNREADABLE_REASON),
        }
    }
    builder.finish()
}

/// The bulk-load format one import reads its objects in.
///
/// The model's `Format` values are `CSV | NTRIPLES | OPEN_CYPHER | PARQUET`
/// (`smithy_query(query="shape",shape="Format")`), and `CSV` "identifies the Gremlin
/// CSV format" (`smithy_query(query="doc",shape="StartImportTaskInput",
/// member="format")`), which is also how the columnar format spells its columns. A
/// request that names no format is read in that same Gremlin convention, which is the
/// one the member's documentation leads with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportFormat {
    /// The Gremlin CSV load format.
    Csv,
    /// The openCypher CSV load format.
    OpenCypher,
    /// RDF N-Triples.
    NTriples,
    /// The columnar load format, whose columns are the Gremlin ones.
    Parquet,
}

impl ImportFormat {
    /// The format the request's `format` member names.
    ///
    /// The member targets the modelled `Format` enum, so the boundary's constraint walk
    /// has already refused any value outside it before a handler runs: what reaches the
    /// fallback is a request that named NO format, which is read in the Gremlin
    /// convention the member's own documentation leads with.
    fn of(format: Option<&str>) -> Self {
        match format.unwrap_or_default() {
            "OPEN_CYPHER" => Self::OpenCypher,
            "NTRIPLES" => Self::NTriples,
            "PARQUET" => Self::Parquet,
            _ => Self::Csv,
        }
    }

    /// Read one object of the source in this format.
    fn read(self, builder: &mut LoadBuilder, object: &[u8]) {
        if self == Self::Parquet {
            load_parquet(builder, LoadColumns::GREMLIN, object);
            return;
        }
        let Ok(text) = ::std::str::from_utf8(object) else {
            // Nothing in a file that is not text can be a row or a triple.
            builder.note_error("A source object is not text.".to_string());
            return;
        };
        match self {
            Self::OpenCypher => load_delimited(builder, LoadColumns::OPEN_CYPHER, text),
            Self::NTriples => load_ntriples(builder, text),
            // The Gremlin convention, which a request naming no format is read in;
            // the columnar format was answered above.
            Self::Csv | Self::Parquet => load_delimited(builder, LoadColumns::GREMLIN, text),
        }
    }
}

/// The S3 export path a Neptune-database import reads its data from, when the request
/// named one.
///
/// "Options for importing data from a Neptune database" with a required `s3ExportPath`
/// — "The path to an S3 bucket from which to import data"
/// (`smithy_query(query="shape",shape="NeptuneImportOptions")`) — so that path is the
/// location the load reads, whatever endpoint or snapshot the `source` names.
fn neptune_export_path(import_options: Option<&ImportOptions>) -> Option<&str> {
    let ImportOptions::Neptune(options) = import_options?;
    Some(options.s3_export_path.as_str())
}

/// The keys one import reads under `prefix` with their LISTED sizes, in listing order,
/// or the `statusReason` the run fails with.
///
/// A source may name one object or a prefix holding many, and a key is a prefix of
/// itself, so both are read with the one listing the prefix gives. A key ending in the
/// delimiter is a folder marker rather than a file of data. The size is carried so the
/// read can refuse an object past its byte bounds BEFORE fetching and buffering any of
/// it.
async fn source_objects(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<(String, usize)>, String> {
    let mut keys: Vec<(String, usize)> = Vec::new();
    // Every key the listing ANSWERS counts against the ceiling, not only the ones read:
    // a prefix of folder markers would otherwise page on without the guard ever firing.
    let mut scanned = 0usize;
    let mut continuation: Option<String> = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_continuation_token(continuation.clone())
            .send()
            .await;
        let page = match page {
            Ok(page) => page,
            Err(error) => {
                // Either the object store answered that it holds nothing at this
                // address, or nothing answered the read at all — and the second is
                // worth a warning, because a deployment with no Amazon S3 mounted to
                // ask is one where no import ever loads anything. The caller's own
                // values are rendered in their escaped `Debug` form, so a bucket or
                // key carrying a newline cannot forge a log line.
                ::tracing::warn!(
                    bucket = ?bucket,
                    prefix = ?prefix,
                    code = ?::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error),
                    "a neptune-graph import task could not list its source in the emulator's Amazon S3"
                );
                return Err(SOURCE_UNREADABLE_REASON.to_string());
            }
        };
        for object in page.contents() {
            let Some(key) = object.key() else { continue };
            scanned += 1;
            if scanned > MAX_IMPORT_SOURCE_OBJECTS {
                return Err(too_many_objects_reason());
            }
            if key.ends_with('/') {
                continue;
            }
            keys.push((
                key.to_string(),
                usize::try_from(object.size().unwrap_or_default().max(0)).unwrap_or(0),
            ));
        }
        match page.next_continuation_token() {
            Some(token) => continuation = Some(token.to_string()),
            None => break,
        }
    }
    Ok(keys)
}

/// The `statusReason` of a run whose source holds more objects than this emulator
/// reads in one import. The bound is the SERVICE's own, so the sentence states it and
/// quotes nothing from the request. Counted over every key the listing answers, so a
/// prefix of folder markers cannot page past it.
fn too_many_objects_reason() -> String {
    format!(
        "The source location holds more than the {MAX_IMPORT_SOURCE_OBJECTS} objects this \
         emulator reads in one import task."
    )
}

/// The `statusReason` of a run whose source holds more bytes than this emulator reads
/// in one import.
fn source_too_large_reason() -> String {
    format!(
        "The source location holds more than the {MAX_IMPORT_SOURCE_BYTES} bytes this emulator \
         reads in one import task."
    )
}

/// An `s3://bucket/key` location split into its bucket and its key prefix, or `None`
/// for a string that names no Amazon S3 location.
fn split_s3_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("s3://")?;
    let (bucket, key) = match rest.split_once('/') {
        Some((bucket, key)) => (bucket, key),
        None => (rest, ""),
    };
    (!bucket.is_empty()).then(|| (bucket.to_string(), key.to_string()))
}
