// SPDX-License-Identifier: CC0-1.0

//! The RUN of one metadata transfer job: reading the source the job names,
//! writing what it describes into the destination, and writing the report of
//! what it did.
//!
//! This is a real run, not a promise. The source is read before anything is
//! written — an Amazon S3 document is fetched through this emulator's OWN object
//! store over the in-process callback, and an IoT TwinMaker workspace is read
//! out of this service's own store — and the destination is written with the
//! entries that read established. So every count the job reports is a
//! measurement of this run, and a job reports `COMPLETED` only for a document
//! the destination really took.
//!
//! The run happens in the call that ACCEPTS the job, which is the one place with
//! a request context to reach another service through; the OUTCOME it
//! establishes is what the clock owes the job
//! ([`METADATA_TRANSFER_JOB_SETTLE_SPAN`]), so a caller still observes the
//! accepted non-terminal state and a poll sees the job settle in the state its
//! run earned.
//!
//! Nothing here opens a socket, resolves a name or reaches anything outside this
//! process: the Amazon S3 client is the one shared constructor every emulated
//! service uses ([`::service_common::s3_client`]), driven over the operation
//! callback, and the endpoint it carries is addressing for the emulator's own
//! router.

use super::*;

/// Why a run could not be performed: the modelled `ErrorCode` the job carries
/// and the sentence that says what the run could not do.
struct Failure {
    code: &'static str,
    message: String,
}

impl Failure {
    /// A run that could not read its source or write its destination.
    fn processing(message: String) -> Self {
        Self {
            code: METADATA_TRANSFER_JOB_ERROR_CODE,
            message,
        }
    }

    /// A run whose source document is no metadata document.
    fn validation(message: String) -> Self {
        Self {
            code: METADATA_TRANSFER_JOB_VALIDATION_ERROR_CODE,
            message,
        }
    }
}

/// Perform the transfer the job `metadata_transfer_job_id` describes, and report
/// the outcome the clock then owes it.
///
/// Answers a [`TransferOutcome`] rather than an error channel: reading the
/// source and writing the destination is the JOB's own work, so what it could
/// not do is the job's own terminal state and `error`, never a refusal of the
/// request that accepted it.
pub async fn perform_transfer<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    metadata_transfer_job_id: &str,
    sources: &[crate::ops::types::SourceConfiguration],
    destination: &crate::ops::types::DestinationConfiguration,
) -> TransferOutcome {
    let document = match read_sources(engine, ctx, region, sources).await {
        Ok(document) => document,
        Err(failure) => {
            // Nothing was read, so there is no entry to count — but there IS a
            // run to report, and the report says why it moved nothing.
            let report = TransferReport::default();
            let report_url = write_report(
                ctx,
                region,
                metadata_transfer_job_id,
                &report,
                METADATA_TRANSFER_JOB_STATE_ERROR,
            )
            .await;
            return TransferOutcome::failed(failure.code, failure.message, None, report_url);
        }
    };
    match write_destination(
        engine,
        ctx,
        region,
        metadata_transfer_job_id,
        destination,
        &document,
    )
    .await
    {
        Ok(report) => {
            let report_url = write_report(
                ctx,
                region,
                metadata_transfer_job_id,
                &report,
                METADATA_TRANSFER_JOB_STATE_COMPLETED,
            )
            .await;
            TransferOutcome::completed(report.progress(), report_url)
        }
        Err(failure) => {
            let report = TransferReport::default();
            let report_url = write_report(
                ctx,
                region,
                metadata_transfer_job_id,
                &report,
                METADATA_TRANSFER_JOB_STATE_ERROR,
            )
            .await;
            TransferOutcome::failed(failure.code, failure.message, None, report_url)
        }
    }
}

/// The document the job's sources describe, or why they describe none.
///
/// `SourceConfigurations` is `@length(1..1)`, so a job has exactly one source;
/// the loop reads every source the request carried and concatenates what they
/// describe, so the rule does not depend on that bound.
async fn read_sources<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    sources: &[crate::ops::types::SourceConfiguration],
) -> Result<MetadataDocument, Failure> {
    let mut document = MetadataDocument::default();
    for source in sources {
        let read = read_source(engine, ctx, region, source).await?;
        document.component_types.extend(read.component_types);
        document.entities.extend(read.entities);
    }
    Ok(document)
}

/// The document ONE source describes, or why it describes none.
async fn read_source<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source: &crate::ops::types::SourceConfiguration,
) -> Result<MetadataDocument, Failure> {
    if let Some(configuration) = &source.s3_configuration {
        return read_s3_document(ctx, region, &configuration.location).await;
    }
    if let Some(configuration) = &source.iot_twin_maker_configuration {
        return read_workspace_document(
            engine,
            region,
            &configuration.workspace,
            configuration.filters.as_deref().unwrap_or_default(),
        );
    }
    // An IoT SiteWise source names asset models, which live in another service
    // this emulator does not mount: there is nothing to read, and a run that
    // read nothing reports the failure that says so rather than a transfer it
    // did not perform.
    Err(Failure::processing(IOT_SITE_WISE_END_MESSAGE.to_string()))
}

/// The metadata document the Amazon S3 object at `location` holds.
///
/// Fetched through this emulator's own object store, so a document a caller
/// staged with `PutObject` is the document this import reads, and at most
/// [`MAX_DOCUMENT_BYTES`] of it is buffered.
async fn read_s3_document(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &str,
) -> Result<MetadataDocument, Failure> {
    let Some((bucket, key)) = split_s3_location(location) else {
        return Err(Failure::processing(format!(
            "The source location {location} names no Amazon S3 object."
        )));
    };
    if key.is_empty() {
        return Err(Failure::processing(format!(
            "The source location {location} names a bucket rather than a metadata document."
        )));
    }
    let read = ::service_common::s3_object_read::read_object(
        ctx.callback(),
        region,
        &bucket,
        &key,
        None,
        MAX_DOCUMENT_BYTES,
    )
    .await;
    match read {
        ::service_common::s3_object_read::ObjectRead::Served(bytes) => {
            super::document::parse(&bytes).map_err(Failure::validation)
        }
        ::service_common::s3_object_read::ObjectRead::Absent => Err(Failure::processing(format!(
            "The source object s3://{bucket}/{key} does not exist."
        ))),
        ::service_common::s3_object_read::ObjectRead::Denied => Err(Failure::processing(format!(
            "The source object s3://{bucket}/{key} may not be read."
        ))),
        ::service_common::s3_object_read::ObjectRead::Unreadable
        | ::service_common::s3_object_read::ObjectRead::Unanswered => {
            Err(Failure::processing(format!(
                "The source object s3://{bucket}/{key} could not be read."
            )))
        }
    }
}

/// The metadata document the workspace `workspace` holds, through the source's
/// own filters.
fn read_workspace_document<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    workspace: &str,
    filters: &[crate::ops::types::IotTwinMakerSourceConfigurationFilter],
) -> Result<MetadataDocument, Failure> {
    let state = service_state(engine).ok_or_else(|| {
        Failure::processing("The source workspace could not be read.".to_string())
    })?;
    let mut state = state.lock();
    let state = state.region_mut(region);
    let workspace_id = state
        .resolve_workspace_id(workspace)
        .ok_or_else(|| Failure::processing(workspace_not_found_message(workspace)))?;
    let record = state
        .workspaces
        .get(&workspace_id)
        .ok_or_else(|| Failure::processing(workspace_not_found_message(&workspace_id)))?;
    Ok(workspace_document(record, filters))
}

/// Write `document` into the job's destination, and report what became of each
/// entry — or why the destination could not be written at all.
async fn write_destination<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    metadata_transfer_job_id: &str,
    destination: &crate::ops::types::DestinationConfiguration,
    document: &MetadataDocument,
) -> Result<TransferReport, Failure> {
    if let Some(configuration) = &destination.iot_twin_maker_configuration {
        return write_into_workspace(engine, region, &configuration.workspace, document);
    }
    if let Some(configuration) = &destination.s3_configuration {
        return write_s3_document(
            ctx,
            region,
            metadata_transfer_job_id,
            &configuration.location,
            document,
        )
        .await;
    }
    // An IoT SiteWise destination would have to create asset models in that
    // service, which this emulator does not mount.
    Err(Failure::processing(IOT_SITE_WISE_END_MESSAGE.to_string()))
}

/// Land `document`'s entries in the destination workspace.
fn write_into_workspace<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    workspace: &str,
    document: &MetadataDocument,
) -> Result<TransferReport, Failure> {
    let state = service_state(engine).ok_or_else(|| {
        Failure::processing("The destination workspace could not be written.".to_string())
    })?;
    let mut state = state.lock();
    let state = state.region_mut(region);
    let workspace_id = state
        .resolve_workspace_id(workspace)
        .ok_or_else(|| Failure::processing(workspace_not_found_message(workspace)))?;
    Ok(write_document_into_workspace(
        state,
        region,
        &workspace_id,
        document,
        ::std::time::SystemTime::now(),
    ))
}

/// Write `document` as the ONE Amazon S3 object the export produces, under the
/// prefix the destination named.
///
/// Every entry the document describes is in that object, so a write that lands
/// transfers all of them; a write that does not land transfers none, which is
/// the failure the job reports.
async fn write_s3_document(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    metadata_transfer_job_id: &str,
    location: &str,
    document: &MetadataDocument,
) -> Result<TransferReport, Failure> {
    let Some((bucket, prefix)) = split_s3_location(location) else {
        return Err(Failure::processing(format!(
            "The destination location {location} names no Amazon S3 bucket."
        )));
    };
    // The destination names a bucket, or a bucket and a prefix inside it; the
    // object the export writes is named for the JOB, so two exports into one
    // prefix do not overwrite each other.
    let key = match prefix.is_empty() {
        true => format!("{metadata_transfer_job_id}{EXPORTED_DOCUMENT_SUFFIX}"),
        false => format!(
            "{}/{metadata_transfer_job_id}{EXPORTED_DOCUMENT_SUFFIX}",
            prefix.trim_end_matches('/')
        ),
    };
    let body = ::serde_json::to_vec(&super::document::render(document)).map_err(|_| {
        Failure::processing("The metadata document could not be written.".to_string())
    })?;
    put_object(ctx, region, &bucket, &key, body)
        .await
        .map_err(|_| {
            Failure::processing(format!(
                "The destination object s3://{bucket}/{key} could not be written."
            ))
        })?;
    // The object holds the whole document, so every entry it describes was
    // written by this run.
    let mut report = TransferReport::default();
    for entry in &document.component_types {
        report.succeeded(ENTRY_KIND_COMPONENT_TYPE, &entry.component_type_id);
    }
    for entry in &document.entities {
        report.succeeded(ENTRY_KIND_ENTITY, &entry.entity_id);
    }
    Ok(report)
}

/// The suffix the one object an export writes carries, after the job's own id.
const EXPORTED_DOCUMENT_SUFFIX: &str = "-metadata.json";

/// The key the run's report is written under, inside the bucket this service
/// keeps its reports in.
fn report_key(metadata_transfer_job_id: &str) -> String {
    format!("{metadata_transfer_job_id}/transfer-report.json")
}

/// Write the run's report, and answer the URL a caller fetches it from — or
/// `None` when there was no object store to write it to.
///
/// `metadataTransferJobRole` is the SERVICE's own role and a job's report is the
/// service's own document, so the report is kept in a bucket of this service
/// rather than in one the caller named: an import names no bucket at all, so
/// there would otherwise be nowhere to put it. The URL is this emulator's own
/// endpoint followed by the object's path, which is the address the emulator's
/// object store serves that object at — never a host only AWS answers.
async fn write_report(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    metadata_transfer_job_id: &str,
    report: &TransferReport,
    state: &str,
) -> Option<String> {
    let bucket = report_bucket(region);
    let key = report_key(metadata_transfer_job_id);
    let body = ::serde_json::to_vec(&report.document(metadata_transfer_job_id, state)).ok()?;
    if !ensure_report_bucket(ctx, region, &bucket).await {
        return None;
    }
    if put_object(ctx, region, &bucket, &key, body).await.is_err() {
        ::tracing::warn!(
            bucket = ?bucket,
            key = ?key,
            "a metadata transfer job's report could not be written to the emulator's Amazon S3"
        );
        return None;
    }
    Some(format!(
        "{}/{bucket}/{key}",
        ctx.endpoint().as_str().trim_end_matches('/')
    ))
}

/// The bucket this service keeps its transfer reports in, for `region`.
///
/// Per Region, the way every other resource of this service is: a job accepted
/// through one Region's endpoint is not one another Region reports, so its
/// report is not an object of another Region's store either. The account
/// segment is the emulator's own, so the name is one bucket per account and
/// Region and is within the 63 characters a bucket name may carry.
#[must_use]
pub fn report_bucket(region: &str) -> String {
    format!(
        "iottwinmaker-transfer-reports-{}-{region}",
        ::emulator_server::emulator_account_id()
    )
}

/// Make sure the report bucket exists, and say whether it does.
///
/// A create of a bucket that is already there answers that the account already
/// owns it, which is the same fact as having created it.
async fn ensure_report_bucket(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> bool {
    let client = ::service_common::s3_client::s3_client(ctx.callback(), region);
    let mut create = client.create_bucket().bucket(bucket);
    // A bucket is created in the Region its location constraint names, and the
    // unspecified constraint stands for us-east-1 alone, so every other Region
    // has to state its own.
    if region != BUCKET_DEFAULT_REGION {
        create = create.create_bucket_configuration(
            ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(region))
                .build(),
        );
    }
    match create.send().await {
        Ok(_) => true,
        Err(error) => {
            // The bucket already being this account's OWN is not a failure: the
            // report has a bucket to go in either way. Any other answer is —
            // including the name being taken by another account, which is a
            // bucket this service may not write to.
            let code = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error);
            code == Some("BucketAlreadyOwnedByYou")
        }
    }
}

/// The one Region whose buckets are created with no location constraint: "if you
/// don't specify a Region, the bucket is created in the US East (N. Virginia)
/// Region (us-east-1) by default"
/// (`CreateBucketConfiguration$LocationConstraint`).
const BUCKET_DEFAULT_REGION: &str = "us-east-1";

/// Put one object into this emulator's own Amazon S3, over the in-process
/// callback.
async fn put_object(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    body: Vec<u8>,
) -> Result<(), ()> {
    let client = ::service_common::s3_client::s3_client(ctx.callback(), region);
    let written = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await;
    match written {
        Ok(_) => Ok(()),
        Err(error) => {
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                code = ?::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error),
                "a metadata transfer job could not write an object to the emulator's Amazon S3"
            );
            Err(())
        }
    }
}

/// The bucket and the key inside it that an `arn:aws:s3:::…` location names, or
/// `None` for a value that names no Amazon S3 address.
///
/// Both modelled locations are ARNs — `S3SourceLocation` names an object
/// (`…:s3:::bucket/key`) and `S3DestinationLocation` a bucket, optionally with a
/// prefix inside it — so one reading serves both. The ARN itself is read by the
/// crate's own ONE parse ([`::service_common::arn::parse_of`]), which is what
/// decides the value is an Amazon S3 ARN at all; what is left here is the split
/// of that ARN's resource into the bucket and the key or prefix inside it
/// (empty when it names the bucket alone).
#[must_use]
pub fn split_s3_location(location: &str) -> Option<(String, String)> {
    let arn = ::service_common::arn::parse_of(location, "s3")?;
    let (bucket, key) = match arn.resource.split_once('/') {
        Some((bucket, key)) => (bucket, key),
        None => (arn.resource, ""),
    };
    (!bucket.is_empty()).then(|| (bucket.to_string(), key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an `arn:aws:s3:::…` location reads as the bucket its first resource
    /// segment names and the key or prefix its remainder names, and a value that
    /// is no such ARN names no address at all.
    #[test]
    fn an_s3_location_reads_as_its_bucket_and_key() {
        // `(case, location, the bucket it names — empty for a value that names
        //  no address at all, the key or prefix inside it)`.
        let rows: &[(&str, &str, &str, &str)] = &[
            (
                "an object in a bucket",
                "arn:aws:s3:::metadata-bucket/entities.json",
                "metadata-bucket",
                "entities.json",
            ),
            (
                "an object under a prefix",
                "arn:aws:s3:::metadata-bucket/exports/2026/entities.json",
                "metadata-bucket",
                "exports/2026/entities.json",
            ),
            (
                "a bucket alone",
                "arn:aws:s3:::export-bucket",
                "export-bucket",
                "",
            ),
            (
                "another partition",
                "arn:aws-us-gov:s3:::export-bucket/out",
                "export-bucket",
                "out",
            ),
            ("no ARN at all", "s3://export-bucket/out", "", ""),
            ("another service's ARN", "arn:aws:iam::0:role/r", "", ""),
            ("an ARN with no resource", "arn:aws:s3:::", "", ""),
        ];
        for (case, location, bucket, key) in rows {
            let actual = split_s3_location(location);
            let expected = (!bucket.is_empty())
                .then(|| ((*bucket).to_string(), (*key).to_string()));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the bucket a run's report is written to is one bucket per account
    /// and Region, named within the length Amazon S3 admits.
    #[test]
    fn the_report_bucket_is_one_per_account_and_region() {
        // The published maximum length of a general purpose bucket name, from
        // the Amazon S3 bucket naming rules.
        const MAX_BUCKET_NAME: usize = 63;
        let rows = ["us-east-1", "us-west-2", "ap-southeast-4"];
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for region in rows {
            let bucket = report_bucket(region);
            assert!(
                bucket.len() <= MAX_BUCKET_NAME
                    && ::service_common::s3_bucket_name::is_well_formed(&bucket),
                "case {region:?}: the report bucket must be a well-formed name, got {bucket:?}"
            );
            assert!(
                seen.insert(bucket.clone()),
                "case {region:?}: each Region keeps its own reports, got {bucket:?}"
            );
        }
    }
}
