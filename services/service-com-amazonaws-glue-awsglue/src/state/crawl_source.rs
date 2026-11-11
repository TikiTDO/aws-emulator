// SPDX-License-Identifier: CC0-1.0

//! Where a CRAWL reads its data from, and how it resolves the objects behind a target into
//! the table the Data Catalog is populated with.
//!
//! A crawl of an Amazon S3 target walks the objects under the target's path and decides
//! three things from what is really there: the dataset's PARTITION KEYS (the folder levels
//! below the target's path, named off a Hive-style `key=value` segment where the path carries
//! one and by depth where it does not), its PARTITIONS (every distinct folder path at that
//! depth) and its COLUMNS (read off the data itself — the header and the first row of a
//! delimited object, or the members of the first JSON document).
//!
//! The objects are read out of the emulator's OWN Amazon S3, through the framework's
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]) — the same
//! transport a client's own S3 request takes, so a crawl classifies exactly the bytes that
//! were uploaded. Nothing here opens a socket, resolves a host or reads an ambient
//! credential.
//!
//! HOW MUCH IS READ IS BOUNDED, before any of it is taken into this process: how many objects
//! the whole walk lists ([`MAX_CRAWLED_OBJECTS`]), how many exclusion patterns one target's
//! objects are matched against ([`MAX_CRAWLED_EXCLUSIONS`], the other factor of that work),
//! how many bytes of one object the schema is inferred from ([`MAX_CRAWLED_SAMPLE_BYTES`]),
//! how long the whole walk may take ([`CRAWL_BUDGET`], which the matching yields to every
//! [`YIELD_EVERY_OBJECTS`] objects) and how many crawls walk at once
//! ([`MAX_CONCURRENT_CRAWLS`]). A crawler whose data is past any of those bounds is one this
//! crawl could not read, which is a crawl that failed rather than one that reports part of
//! the data as the whole of it.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

use super::*;

/// How many objects ONE CRAWL lists, across every target it walks.
///
/// CHOSEN, not an AWS quota: a crawl walks the objects under the prefixes its targets name,
/// and both the prefixes and what is under them are written by callers, so the walk is
/// bounded here — across the whole crawl rather than per target, because a crawler names as
/// many targets as a create cared to give it. A crawler whose targets hold more is data this
/// crawl could not read, which is a crawl that failed rather than one reporting part of the
/// data as the whole of it.
pub const MAX_CRAWLED_OBJECTS: usize = 1_000;

/// How many bytes of one object a dataset's schema is inferred from, asked for as a RANGE so
/// the bound is enforced by the request and nothing larger is buffered.
///
/// CHOSEN, for the reason [`MAX_CRAWLED_OBJECTS`] gives. A schema is read off the first rows
/// of an object, so this is far more than the inference needs.
pub const MAX_CRAWLED_SAMPLE_BYTES: usize = 1024 * 1024;

/// How many of ONE target's exclusion patterns a crawl compiles and matches its objects
/// against.
///
/// CHOSEN, not an AWS quota: `S3Target$Exclusions` carries no modelled length, and the work a
/// crawl does matching it is the PRODUCT of that length and the objects it listed, so the
/// factor the model leaves open is bounded here — the way [`MAX_CRAWLED_OBJECTS`] bounds the
/// other one. A target carrying more patterns is one this crawl did not read.
pub const MAX_CRAWLED_EXCLUSIONS: usize = 50;

/// How often the walk hands the runtime back while it matches a target's objects against its
/// exclusion patterns: once per this many objects.
///
/// The matching is this emulator's own synchronous work on the request's task, so it yields
/// often enough that [`CRAWL_BUDGET`] and a cancellation can both land inside the matching
/// region, and seldom enough that the yield is not the cost of an object.
pub const YIELD_EVERY_OBJECTS: usize = 200;

/// How long ONE crawl may spend walking its targets and classifying their data.
///
/// CHOSEN, not an AWS duration: it is the wall clock this emulator gives its own walk, so
/// targets that cannot be walked inside it end the crawl rather than holding a request open.
pub const CRAWL_BUDGET: Duration = Duration::from_secs(10);

/// How many crawls walk their targets at once.
///
/// CHOSEN: a crawl is in flight per crawler, so without this the bounds above would be
/// multiplied by the number of crawlers an account holds. Two crawls walk at a time and the
/// rest wait for a permit.
pub const MAX_CONCURRENT_CRAWLS: usize = 2;

/// How long a crawl waits for one of [`MAX_CONCURRENT_CRAWLS`].
///
/// CHOSEN: the wait is bounded, so a start answers either way rather than queueing without
/// end; a crawl that waited this long reports that nothing read its data.
pub const CRAWL_PERMIT_WAIT: Duration = Duration::from_secs(5);

/// The permits [`MAX_CONCURRENT_CRAWLS`] hands out, for the whole process: the bound is on
/// how much data this emulator reads at once, whichever crawler or caller asked.
static CRAWL_PERMITS: ::tokio::sync::Semaphore =
    ::tokio::sync::Semaphore::const_new(MAX_CONCURRENT_CRAWLS);

/// The field delimiters a delimited-text object is classified by, in the order they are
/// tried.
///
/// The set Glue's own built-in CSV classifier checks for: a comma, a pipe, a tab, a
/// semicolon and Ctrl-A, which is Hive's own default field separator.
pub const CANDIDATE_DELIMITERS: [u8; 5] = [b',', b'|', b'\t', b';', 0x01];

/// How many characters a cell of a HEADER row may hold, except in the last column.
///
/// One of the conditions Glue's built-in CSV classifier applies when it decides whether the
/// first row of an object is a header rather than data.
pub const MAX_HEADER_CELL_LENGTH: usize = 150;

/// How deep a JSON value's type is rendered before the rest of it is reported as text.
///
/// CHOSEN: a JSON document is written by callers, so the nesting a column type renders is
/// bounded here rather than by the document.
pub const MAX_JSON_TYPE_DEPTH: usize = 8;

/// `Column$Type` of a JSON member holding a whole number inside a 32-bit integer.
pub const JSON_INT_TYPE: &str = "int";
/// `Column$Type` of a JSON member holding a whole number outside it, and of a delimited cell
/// holding one.
pub const BIGINT_TYPE: &str = "bigint";
/// `Column$Type` of a member or cell holding a number that is not whole.
pub const DOUBLE_TYPE: &str = "double";
/// `Column$Type` of a member or cell holding a boolean.
pub const BOOLEAN_TYPE: &str = "boolean";
/// `Column$Type` of a member or cell holding text, and of one nothing could be inferred
/// from.
pub const STRING_TYPE: &str = "string";

/// How a column is named when the data carries no header to name it with: `col0`, `col1`, …
/// by its position, which is how Glue names the columns of a headerless delimited object.
pub const UNNAMED_COLUMN_PREFIX: &str = "col";

/// `LastCrawlInfo$ErrorMessage` of a crawl that was not carried out because too many were
/// already walking their targets: the condition is on the SERVICE, not on the crawler, so the
/// reason says so rather than claiming the data could not be read.
pub const CRAWL_TOO_MANY_RUNNING: &str =
    "Too many crawls are in progress, so this crawl did not read its targets.";

/// `LastCrawlInfo$ErrorMessage` of a crawl that ran out of time before it had walked its
/// targets. Also a condition on the crawl rather than on the data.
pub const CRAWL_TIMED_OUT: &str = "The crawl timed out before it finished reading its targets.";

/// `LastCrawlInfo$ErrorMessage` of a crawl whose target names no Amazon S3 location.
///
/// The variable part is the path the crawl would have read, read off the stored crawler.
pub fn crawl_target_is_not_an_s3_location(path: &str) -> String {
    format!("The crawl target {path} does not name an Amazon S3 location.")
}

/// `LastCrawlInfo$ErrorMessage` of a crawl whose target could not be read: the object store
/// answered that it does not serve it, or it holds more data than one crawl reads.
///
/// The variable part is the path the crawl was reading. WHY it could not be read is logged
/// against the crawl rather than reported to the caller.
pub fn crawl_target_unreadable(path: &str) -> String {
    format!("The crawl target {path} could not be read, so no table was created for it.")
}

/// Walk the crawler's data stores and answer what the crawl found.
///
/// BOUNDED in every direction a caller can grow: how many crawls walk at once
/// ([`MAX_CONCURRENT_CRAWLS`], with a bounded wait for a permit), how long one crawl may take
/// ([`CRAWL_BUDGET`], which the walk yields to while it matches), how many objects the whole
/// walk lists ([`MAX_CRAWLED_OBJECTS`]), how many exclusion patterns they are matched against
/// ([`MAX_CRAWLED_EXCLUSIONS`]) and how many bytes of one object the schema is read from
/// ([`MAX_CRAWLED_SAMPLE_BYTES`]).
pub async fn perform_crawl(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    plan: &CrawlPlan,
) -> CrawlOutcome {
    let permit =
        match ::tokio::time::timeout(CRAWL_PERMIT_WAIT, CRAWL_PERMITS.acquire()).await {
            Ok(Ok(permit)) => permit,
            _ => {
                ::tracing::debug!(
                    crawler = ?plan.crawler_name,
                    "a Glue crawl found no free reader for its targets"
                );
                return CrawlOutcome::Failed(CRAWL_TOO_MANY_RUNNING.to_string());
            }
        };
    let client = super::column_statistics_source::s3_client(ctx, region);
    let walked = ::tokio::time::timeout(CRAWL_BUDGET, walk_targets(&client, plan)).await;
    drop(permit);

    match walked {
        Ok(Ok(discovered)) => CrawlOutcome::Discovered(discovered),
        Ok(Err(reason)) => CrawlOutcome::Failed(reason),
        // The crawl ran out of time rather than out of readable data, so it says so.
        Err(_elapsed) => {
            ::tracing::debug!(
                crawler = ?plan.crawler_name,
                "a Glue crawl did not read its targets inside its budget"
            );
            CrawlOutcome::Failed(CRAWL_TIMED_OUT.to_string())
        }
    }
}

/// One dataset per Amazon S3 target that holds data, in the order the crawler names them.
///
/// A target that holds no object at all yields no dataset — there is nothing there to
/// describe — and so does one whose objects nothing could be classified from: a crawl writes
/// a table for data it read, never an empty table for data it could not.
async fn walk_targets(
    client: &::aws_sdk_s3::Client,
    plan: &CrawlPlan,
) -> Result<Vec<DiscoveredTable>, String> {
    let mut discovered: Vec<DiscoveredTable> = Vec::new();
    let mut named: HashSet<String> = HashSet::new();
    // How many objects the whole crawl may still list: one budget across the targets, charged
    // by what each listing really returned.
    let mut budget = MAX_CRAWLED_OBJECTS;
    for target in &plan.s3_targets {
        let Some(path) = target
            .path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        let Some((bucket, prefix)) = split_s3_location(path) else {
            return Err(crawl_target_is_not_an_s3_location(path));
        };
        // A target names a FOLDER, so the objects of its dataset are the ones under it: the
        // prefix every key of the dataset starts with, and the root every relative path is
        // taken from.
        let root = folder_prefix(&prefix);
        let listed = match list_objects(client, &bucket, &root, budget).await {
            Ok(listed) => listed,
            Err(detail) => {
                // The target's own value is rendered in its escaped `Debug` form, so a path
                // carrying a newline cannot forge a log line.
                ::tracing::debug!(
                    target = ?path,
                    detail = %detail,
                    "a Glue crawl could not read the objects under one of its targets"
                );
                return Err(crawl_target_unreadable(path));
            }
        };
        budget = budget.saturating_sub(listed.len());
        let objects = match retained(listed, &root, target).await {
            Ok(objects) => objects,
            Err(detail) => {
                ::tracing::debug!(
                    target = ?path,
                    detail = %detail,
                    "a Glue crawl could not match the objects under one of its targets"
                );
                return Err(crawl_target_unreadable(path));
            }
        };
        if objects.is_empty() {
            continue;
        }
        let Some(name) = dataset_name(&bucket, &root) else {
            continue;
        };
        let Some(dataset) =
            describe_dataset(client, &bucket, &root, name, &objects).await?
        else {
            continue;
        };
        // Two targets that resolve to the same table name describe one dataset, and the
        // first of them is the one that described it.
        if named.insert(dataset.name.clone()) {
            discovered.push(dataset);
        }
    }
    Ok(discovered)
}

/// One object a crawl listed: its key, its size, and the relative path that places it under
/// the target's root.
#[derive(Debug, Clone)]
struct CrawledObject {
    /// The object's key in the bucket.
    key: String,
    /// What the listing declared the object's size to be.
    size: u64,
    /// The folder levels between the target's root and the object itself.
    folders: Vec<String>,
}

/// The keys and sizes under `prefix`, bounded by the objects this crawl may still list
/// (`budget`, out of [`MAX_CRAWLED_OBJECTS`] for the whole walk).
///
/// `Err` is why the data could not be read, for the crawl's log.
async fn list_objects(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
    budget: usize,
) -> Result<Vec<(String, u64)>, String> {
    let mut listed: Vec<(String, u64)> = Vec::new();
    let mut continuation: Option<String> = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_continuation_token(continuation.clone())
            .send()
            .await
            .map_err(|error| {
                format!(
                    "the object store answered {:?} to the listing: {}",
                    error.code(),
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                )
            })?;
        for object in page.contents() {
            let Some(key) = object.key() else { continue };
            // A key ending in the layout's own separator is a folder marker, not data.
            if key.ends_with('/') {
                continue;
            }
            let size = u64::try_from(object.size().unwrap_or_default().max(0)).unwrap_or(0);
            listed.push((key.to_string(), size));
            if listed.len() > budget {
                return Err(format!(
                    "the crawler's targets hold more than the {MAX_CRAWLED_OBJECTS} objects one \
                     crawl lists"
                ));
            }
        }
        match page.next_continuation_token() {
            Some(token) => continuation = Some(token.to_string()),
            None => break,
        }
    }
    // One deterministic order, so the object a schema is inferred from and the partitions a
    // dataset reports do not depend on how the listing paged.
    listed.sort();
    Ok(listed)
}

/// The listed objects this crawl keeps: the ones the target's `Exclusions` do not exclude,
/// at most `SampleSize` of them per leaf folder.
///
/// "A list of glob patterns used to exclude from the crawl" (`S3Target$Exclusions`) is
/// matched against the object's path RELATIVE to the target's own root, which is what the
/// patterns are written against. A pattern that is not a glob excludes nothing: the crawl
/// reads the data rather than failing on a value it cannot compile.
///
/// HOW MANY patterns are compiled and matched is bounded ([`MAX_CRAWLED_EXCLUSIONS`]): the
/// list is written by callers and the model constrains its length nowhere, and the work here
/// is the PRODUCT of that length and the objects listed — so a target carrying more patterns
/// is one this crawl did not read, rather than one whose patterns were silently dropped. The
/// walk hands the runtime back every [`YIELD_EVERY_OBJECTS`] objects, so the crawl's budget
/// and a cancellation can land inside the matching region rather than only at the object
/// store's own awaits.
///
/// "Sets the number of files in each leaf folder to be crawled when crawling sample files in
/// a dataset. If not set, all the files are crawled" (`S3Target$SampleSize`) — so the bound
/// is per leaf folder, applied in the listing's own order.
async fn retained(
    listed: Vec<(String, u64)>,
    root: &str,
    target: &S3Target,
) -> Result<Vec<CrawledObject>, String> {
    let patterns = target.exclusions.as_ref().map_or(0, Vec::len);
    if patterns > MAX_CRAWLED_EXCLUSIONS {
        return Err(format!(
            "the target carries {patterns} exclusion patterns, more than the \
             {MAX_CRAWLED_EXCLUSIONS} one crawl matches its objects against"
        ));
    }
    let exclusions: Vec<::globset::GlobMatcher> = target
        .exclusions
        .iter()
        .flatten()
        .filter_map(|pattern| {
            ::globset::Glob::new(pattern)
                .ok()
                .map(|glob| glob.compile_matcher())
        })
        .collect();
    let sample_size = target
        .sample_size
        .filter(|size| *size > 0)
        .and_then(|size| usize::try_from(size).ok());
    let mut per_folder: HashMap<Vec<String>, usize> = HashMap::new();
    let mut kept: Vec<CrawledObject> = Vec::new();
    for (walked, (key, size)) in listed.into_iter().enumerate() {
        // The matching is this emulator's own synchronous work on the request's task, so it
        // hands the runtime back often enough for the crawl's budget, a cancellation and
        // every other request to land while it goes on, and seldom enough that the yield is
        // not the cost of an object.
        if walked.is_multiple_of(YIELD_EVERY_OBJECTS) {
            ::tokio::task::yield_now().await;
        }
        let Some(relative) = key.strip_prefix(root) else {
            continue;
        };
        if relative.is_empty() {
            continue;
        }
        if exclusions
            .iter()
            .any(|exclusion| exclusion.is_match(relative))
        {
            continue;
        }
        let mut segments: Vec<&str> = relative.split('/').collect();
        // The last segment is the object's own name; the ones before it are the folder
        // levels that place it under the root.
        segments.pop();
        let folders: Vec<String> = segments.into_iter().map(str::to_string).collect();
        if let Some(bound) = sample_size {
            let taken = per_folder.entry(folders.clone()).or_default();
            if *taken >= bound {
                continue;
            }
            *taken += 1;
        }
        kept.push(CrawledObject {
            key,
            size,
            folders,
        });
    }
    Ok(kept)
}

/// The dataset behind one target: its partition keys, its partitions and the columns its
/// data carries, or `None` when nothing could be classified from its objects.
async fn describe_dataset(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    root: &str,
    name: String,
    objects: &[CrawledObject],
) -> Result<Option<DiscoveredTable>, String> {
    let (partition_keys, partitions) = partitioning(bucket, root, objects);
    // The schema is inferred from the first object that holds any bytes: an empty object
    // carries no row to read a column off.
    let Some(sample) = objects.iter().find(|object| object.size > 0) else {
        return Ok(None);
    };
    let bytes = read_sample(client, bucket, &sample.key).await?;
    let Some(schema) = infer_schema(&bytes) else {
        ::tracing::debug!(
            object = ?sample.key,
            "a Glue crawl could not classify the data under one of its targets"
        );
        return Ok(None);
    };
    Ok(Some(DiscoveredTable {
        name,
        location: s3_url(bucket, root),
        classification: schema.classification,
        serialization_library: schema.serialization_library,
        delimiter: schema.delimiter,
        header_lines: schema.header_lines,
        columns: schema.columns,
        partition_keys,
        partitions,
        object_count: u64::try_from(objects.len()).unwrap_or(u64::MAX),
        size_bytes: objects.iter().map(|object| object.size).sum(),
    }))
}

/// The partition keys and partitions of a dataset, read off the folder levels its objects sit
/// at.
///
/// The DEPTH is the smallest number of folder levels any of the objects sits at, so every
/// object lies within exactly one partition — a dataset with an object directly at its root
/// is not partitioned at all. A level whose segments all read `name=value` under ONE name is
/// a partition key of that name carrying those values, which is the Hive layout; any other
/// level is a key named by its depth ([`UNNAMED_PARTITION_KEY_PREFIX`]) carrying the whole
/// segment.
fn partitioning(
    bucket: &str,
    root: &str,
    objects: &[CrawledObject],
) -> (Vec<Column>, Vec<DiscoveredPartition>) {
    let depth = objects
        .iter()
        .map(|object| object.folders.len())
        .min()
        .unwrap_or(0);
    if depth == 0 {
        return (Vec::new(), Vec::new());
    }
    let mut keys: Vec<Column> = Vec::with_capacity(depth);
    // Whether each level is a Hive-style `name=value` level, decided once for the level
    // rather than per object: the key's name has to be the same for every partition of it.
    let mut hive_level: Vec<bool> = Vec::with_capacity(depth);
    for level in 0..depth {
        let mut names = objects.iter().map(|object| hive_key(&object.folders[level]));
        let first = names.next().flatten();
        let shared = match first {
            Some(name) if names.all(|other| other.as_deref() == Some(name.as_str())) => Some(name),
            _ => None,
        };
        hive_level.push(shared.is_some());
        let name = shared
            .and_then(|name| catalog_name(&name))
            .unwrap_or_else(|| format!("{UNNAMED_PARTITION_KEY_PREFIX}{level}"));
        keys.push(Column {
            name,
            r#type: Some(PARTITION_KEY_TYPE.to_string()),
            ..Column::default()
        });
    }

    // Ordered and deduplicated by the values that address the partition, so one folder path
    // is one partition however many objects sit under it.
    let mut partitions: BTreeMap<Vec<String>, DiscoveredPartition> = BTreeMap::new();
    for object in objects {
        let segments = &object.folders[..depth];
        let values: Vec<String> = segments
            .iter()
            .enumerate()
            .map(|(level, segment)| {
                if hive_level[level] {
                    hive_value(segment).to_string()
                } else {
                    segment.clone()
                }
            })
            .collect();
        let location = s3_url(bucket, &format!("{root}{}/", segments.join("/")));
        partitions
            .entry(values.clone())
            .or_insert(DiscoveredPartition { values, location });
    }
    (keys, partitions.into_values().collect())
}

/// The key name of a Hive-style `name=value` path segment, or `None` for a segment that is
/// not one.
fn hive_key(segment: &str) -> Option<String> {
    let (name, _) = segment.split_once('=')?;
    (!name.is_empty()).then(|| name.to_string())
}

/// The value of a Hive-style `name=value` path segment, or the whole segment when it is not
/// one.
fn hive_value(segment: &str) -> &str {
    match segment.split_once('=') {
        Some((_, value)) => value,
        None => segment,
    }
}

/// The bytes a dataset's schema is inferred from, bounded by [`MAX_CRAWLED_SAMPLE_BYTES`]:
/// the read asks for at most that many, so the bound is enforced by the REQUEST and nothing
/// larger is buffered.
async fn read_sample(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> Result<Vec<u8>, String> {
    let object = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .range(format!("bytes=0-{}", MAX_CRAWLED_SAMPLE_BYTES - 1))
        .send()
        .await
        .map_err(|error| {
            format!(
                "the object store answered {:?} for the object {key:?}: {}",
                error.code(),
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    let body = object
        .body
        .collect()
        .await
        .map_err(|error| format!("the object {key:?} did not arrive in full: {error}"))?
        .into_bytes();
    Ok(body.to_vec())
}

/// What a dataset's objects were classified as: the layout, and the columns it carries.
#[derive(Debug, Clone)]
struct InferredSchema {
    /// The value the table's [`CLASSIFICATION_PARAMETER`] carries.
    classification: String,
    /// The serde the layout is read with.
    serialization_library: String,
    /// The field delimiter of a delimited layout.
    delimiter: Option<String>,
    /// How many leading lines of each object are a header.
    header_lines: usize,
    /// The columns, in the order the data carries them.
    columns: Vec<Column>,
}

/// Classify one object's bytes and read the dataset's columns off them, or `None` for bytes
/// that are neither JSON documents nor delimited text.
fn infer_schema(bytes: &[u8]) -> Option<InferredSchema> {
    let text = ::std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next()?;
    let second = lines.next();
    // A JSON object per line is recognised by parsing the first one: an object is the only
    // JSON document whose members are the columns of a table.
    if let Some(members) = ::serde_json::from_str::<::serde_json::Value>(first.trim())
        .ok()
        .and_then(|document| match document {
            ::serde_json::Value::Object(members) => Some(members),
            _ => None,
        })
    {
        let columns: Vec<Column> = members
            .iter()
            .filter_map(|(member, value)| {
                catalog_name(member).map(|name| Column {
                    name,
                    r#type: Some(json_column_type(value, 0)),
                    ..Column::default()
                })
            })
            .collect();
        if columns.is_empty() {
            return None;
        }
        return Some(InferredSchema {
            classification: CLASSIFICATION_JSON.to_string(),
            serialization_library: JSON_SERDE.to_string(),
            // A JSON document names its own members, so the layout has neither a field
            // delimiter nor a header line.
            delimiter: None,
            header_lines: 0,
            columns,
        });
    }

    let delimiter = delimiter_of(first);
    let header_cells = delimited_cells(first, delimiter);
    if header_cells.is_empty() {
        return None;
    }
    let data_cells = second.map(|line| delimited_cells(line, delimiter));
    let header = is_header(&header_cells, data_cells.as_deref());
    // The row the column types are read from: the first row that is not the header.
    let typed = if header {
        data_cells.as_deref()
    } else {
        Some(header_cells.as_slice())
    };
    let columns: Vec<Column> = header_cells
        .iter()
        .enumerate()
        .map(|(position, cell)| {
            let name = if header {
                catalog_name(cell)
                    .unwrap_or_else(|| format!("{UNNAMED_COLUMN_PREFIX}{position}"))
            } else {
                format!("{UNNAMED_COLUMN_PREFIX}{position}")
            };
            Column {
                name,
                r#type: Some(
                    typed
                        .and_then(|row| row.get(position))
                        .map_or(STRING_TYPE, |cell| delimited_column_type(cell))
                        .to_string(),
                ),
                ..Column::default()
            }
        })
        .collect();
    Some(InferredSchema {
        classification: CLASSIFICATION_CSV.to_string(),
        serialization_library: LAZY_SIMPLE_SERDE.to_string(),
        delimiter: Some(char::from(delimiter).to_string()),
        header_lines: usize::from(header),
        columns,
    })
}

/// The field delimiter of a delimited row: of [`CANDIDATE_DELIMITERS`], the first that splits
/// the row into more than one field, else a comma.
///
/// A row that no candidate splits is a single-column row, which the comma reads exactly as
/// well as any other delimiter would.
fn delimiter_of(row: &str) -> u8 {
    CANDIDATE_DELIMITERS
        .into_iter()
        .find(|candidate| delimited_cells(row, *candidate).len() > 1)
        .unwrap_or(b',')
}

/// One delimited row's cells, read the way the serde reads them — quoted fields included,
/// which is what the `csv` reader is for.
fn delimited_cells(row: &str, delimiter: u8) -> Vec<String> {
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(row.as_bytes());
    reader
        .records()
        .next()
        .and_then(Result::ok)
        .map(|record| {
            record
                .iter()
                .map(|cell| cell.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the first row of a delimited object is a HEADER rather than data.
///
/// The conditions Glue's built-in CSV classifier applies: every cell of the row has content,
/// every cell parses as text rather than as a number or a boolean, every cell but the last is
/// shorter than [`MAX_HEADER_CELL_LENGTH`], and the row differs from the row after it. A
/// single-row object has no row to differ from, so its one row is data.
fn is_header(first: &[String], second: Option<&[String]>) -> bool {
    let Some(second) = second else {
        return false;
    };
    if first == second {
        return false;
    }
    let last = first.len().saturating_sub(1);
    first.iter().enumerate().all(|(position, cell)| {
        !cell.is_empty()
            && delimited_column_type(cell) == STRING_TYPE
            && (position == last || cell.chars().count() < MAX_HEADER_CELL_LENGTH)
    })
}

/// The `Column$Type` one delimited cell's text reads as: a whole number, a number that is not
/// whole, a boolean, else text — which is also what an empty cell reads as, because nothing
/// can be inferred from it.
fn delimited_column_type(cell: &str) -> &'static str {
    if cell.is_empty() {
        return STRING_TYPE;
    }
    if cell.parse::<i64>().is_ok() {
        return BIGINT_TYPE;
    }
    if cell.parse::<f64>().is_ok() {
        return DOUBLE_TYPE;
    }
    if cell.eq_ignore_ascii_case("true") || cell.eq_ignore_ascii_case("false") {
        return BOOLEAN_TYPE;
    }
    STRING_TYPE
}

/// The `Column$Type` one JSON member's value reads as.
///
/// A whole number inside a 32-bit integer is that integer type and a wider one is the long
/// type; an array is the type of its first element and an object the struct of its members,
/// each rendered the way Hive spells a nested type. A value nothing can be inferred from —
/// JSON's own null, an empty array — reads as text, and so does everything past
/// [`MAX_JSON_TYPE_DEPTH`] levels of nesting.
fn json_column_type(value: &::serde_json::Value, depth: usize) -> String {
    if depth >= MAX_JSON_TYPE_DEPTH {
        return STRING_TYPE.to_string();
    }
    match value {
        ::serde_json::Value::Null | ::serde_json::Value::String(_) => STRING_TYPE.to_string(),
        ::serde_json::Value::Bool(_) => BOOLEAN_TYPE.to_string(),
        ::serde_json::Value::Number(number) => match number.as_i64() {
            Some(whole) if i32::try_from(whole).is_ok() => JSON_INT_TYPE.to_string(),
            Some(_) => BIGINT_TYPE.to_string(),
            None => DOUBLE_TYPE.to_string(),
        },
        ::serde_json::Value::Array(elements) => {
            let element = elements
                .first()
                .map_or_else(|| STRING_TYPE.to_string(), |first| json_column_type(first, depth + 1));
            format!("array<{element}>")
        }
        ::serde_json::Value::Object(members) => {
            let rendered: Vec<String> = members
                .iter()
                .filter_map(|(member, value)| {
                    catalog_name(member)
                        .map(|name| format!("{name}:{}", json_column_type(value, depth + 1)))
                })
                .collect();
            format!("struct<{}>", rendered.join(","))
        }
    }
}

/// The name the catalog holds a crawled column or partition key under: the data's own name
/// lowercased, with every character the catalog does not hold a name in replaced by an
/// underscore.
///
/// `None` for a name there is nothing left of, which is not a name the catalog can address a
/// column by.
fn catalog_name(name: &str) -> Option<String> {
    let folded: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    (!folded.is_empty() && folded.chars().any(|character| character != '_')).then_some(folded)
}

/// The name of the dataset behind a target: the last folder level of its path, else — for a
/// target that names a whole bucket — the bucket's own name, in both cases as the catalog
/// holds a table name ([`catalog_name`]).
fn dataset_name(bucket: &str, root: &str) -> Option<String> {
    let trimmed = root.trim_end_matches('/');
    let last = trimmed
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(bucket);
    catalog_name(last)
}

/// The prefix every key of a target's dataset starts with: the target's path as a FOLDER, so
/// a value that does not end in the separator still names the folder it addresses, and a
/// target that names a whole bucket has no prefix at all.
fn folder_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

/// The `s3://` location of one prefix inside a bucket, which is what a table's and a
/// partition's `StorageDescriptor$Location` carries.
fn s3_url(bucket: &str, prefix: &str) -> String {
    format!("s3://{bucket}/{prefix}")
}
