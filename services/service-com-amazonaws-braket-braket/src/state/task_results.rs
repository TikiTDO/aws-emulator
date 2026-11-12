// SPDX-License-Identifier: CC0-1.0

//! The result document of a quantum task, and the Amazon S3 bucket it is WRITTEN to.
//!
//! The Braket data plane is the API plus Amazon S3: a client reads a task's measurements
//! out of the object the service writes under the task's `outputS3Directory` in the
//! bucket the create named. So the bucket belongs to Amazon S3, and so does the result
//! object:
//!
//! * the create RESOLVES the named bucket where S3 holds it, and refuses the request when
//!   S3 reports no such bucket ([`output_bucket_is_absent`]);
//! * a task whose program ran has its result document WRITTEN there before it is reported
//!   as completed ([`write_result_document`]), so a task this service reports as
//!   `COMPLETED` is one whose results a client can actually read.
//!
//! Amazon S3 is reached the way the framework documents a handler reaching another
//! emulated service ([`smithy_server_runtime::OperationContext::callback`]), through the
//! tree's one construction of that client (`service_common::s3_client`), so nothing here
//! opens a socket, leaves the process or names another service crate. Only S3's own
//! ANSWER refuses anything: a read that reports no such bucket is what refuses a create,
//! while an emulator assembled without Amazon S3 to ask establishes nothing about the
//! bucket and so refuses nothing (`services/common/src/network.rs` states which
//! resolution is which). A write that could not be made is not a result that exists,
//! which is why its failure is what the task reports.

use super::quantum_simulation::TaskMeasurements;

/// The object a gate-model task's results are written as, under the task's
/// `outputS3Directory` — the name the Amazon Braket SDK reads a finished task's results
/// from.
pub const RESULTS_OBJECT: &str = "results.json";

/// The `braketSchemaHeader` name of a gate-model task's result document, and of the
/// metadata documents nested in it, as the Braket schemas publish them.
const GATE_MODEL_RESULT_SCHEMA: &str = "braket.task_result.gate_model_task_result";
const TASK_METADATA_SCHEMA: &str = "braket.task_result.task_metadata";
const ADDITIONAL_METADATA_SCHEMA: &str = "braket.task_result.additional_metadata";
const SIMULATOR_METADATA_SCHEMA: &str = "braket.task_result.simulator_metadata";

/// The version every one of those schemas is published at.
const SCHEMA_VERSION: &str = "1";

/// The media type of the result object.
const RESULTS_CONTENT_TYPE: &str = "application/json";

/// How Amazon Braket refuses a create whose `outputS3Bucket` it cannot resolve for the
/// caller: the fixed sentence pointing at the developer guide, interpolating nothing from
/// the request.
pub const INACCESSIBLE_BUCKET_REFUSAL: &str = "Caller doesn't have access to bucket or it \
doesn't exist. Learn more on our developer guide: \
https://docs.aws.amazon.com/braket/latest/developerguide/braket-slr.html";

/// Why a task whose program ran still has no result to read: the write of its result
/// document could not be made, so the output location holds nothing.
#[must_use]
pub fn unwritten_results_reason(bucket: &str, key: &str) -> String {
    format!(
        "The quantum task's result document could not be written to s3://{bucket}/{key}, so \
         no result was stored for it."
    )
}

/// Whether Amazon S3 reports holding no bucket named `bucket` in `region`.
///
/// Asked with `HeadBucket`, the operation S3 documents for exactly this ("You can use
/// this operation to determine if a bucket exists"): its `NotFound` answer is S3 saying
/// the bucket is not there. Every other failure leaves the question unanswered, so it
/// reports the bucket as present and refuses nothing.
///
/// Call it with NO guard of this service's store held: it awaits.
pub async fn output_bucket_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> bool {
    let answer = s3_client(ctx, region)
        .head_bucket()
        .bucket(bucket)
        .send()
        .await;
    match answer {
        Ok(_) => false,
        Err(error)
            if error.as_service_error().is_some_and(
                ::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found,
            ) =>
        {
            true
        }
        Err(error) => {
            unresolved(bucket, &error);
            false
        }
    }
}

/// Write `document` as the object `key` of `bucket`, and report whether the write was
/// made.
///
/// Call it with NO guard of this service's store held: it awaits.
pub async fn write_result_document(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    document: &::serde_json::Value,
) -> bool {
    let body = document.to_string();
    let written = s3_client(ctx, region)
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(RESULTS_CONTENT_TYPE)
        .body(::aws_sdk_s3::primitives::ByteStream::from(body.into_bytes()))
        .send()
        .await;
    match written {
        Ok(_) => true,
        Err(error) => {
            unwritten(bucket, key, &error);
            false
        }
    }
}

/// Everything the result document reports about the run it belongs to.
#[derive(Debug, Clone, Copy)]
pub struct ResultContext<'r> {
    /// The task's own ARN, which the document carries as its id.
    pub quantum_task_arn: &'r str,
    /// The device that ran it.
    pub device_arn: &'r str,
    /// The `action` document the task was created with, as it was sent.
    pub action: &'r str,
    pub shots: i64,
    /// When the task was created, and when its run ended, as ISO-8601 instants.
    pub created_at: &'r str,
    pub ended_at: &'r str,
    /// The status the task reaches once this document is written.
    pub status: &'r str,
}

/// The result document of a gate-model run: the measurements it produced, the qubits they
/// are the values of, and the metadata of the task and the simulator that ran it.
///
/// Every value is read from the run or from the task's own record — nothing is invented:
/// `measurements` holds one row per shot as the simulator drew it, and the simulator's
/// `executionDuration` is how long the simulation actually took, in milliseconds.
#[must_use]
pub fn gate_model_result(
    context: ResultContext<'_>,
    measurements: &TaskMeasurements,
) -> ::serde_json::Value {
    let action: ::serde_json::Value = ::serde_json::from_str(context.action)
        .unwrap_or_else(|_| ::serde_json::Value::String(context.action.to_string()));
    ::serde_json::json!({
        "braketSchemaHeader": {
            "name": GATE_MODEL_RESULT_SCHEMA,
            "version": SCHEMA_VERSION,
        },
        // Borrowed rather than cloned: the measurement matrix is the largest thing the
        // document carries, so it is serialised from the run's own rows.
        "measurements": &measurements.rows,
        "measuredQubits": &measurements.measured_qubits,
        // A shot-based run reports the measurements themselves; no result type was
        // computed, so the list of them is empty rather than filled with values nothing
        // asked for.
        "resultTypes": [],
        "taskMetadata": {
            "braketSchemaHeader": {
                "name": TASK_METADATA_SCHEMA,
                "version": SCHEMA_VERSION,
            },
            "id": context.quantum_task_arn,
            "deviceId": context.device_arn,
            "shots": context.shots,
            "createdAt": context.created_at,
            "endedAt": context.ended_at,
            "status": context.status,
        },
        "additionalMetadata": {
            "braketSchemaHeader": {
                "name": ADDITIONAL_METADATA_SCHEMA,
                "version": SCHEMA_VERSION,
            },
            "action": action,
            "simulatorMetadata": {
                "braketSchemaHeader": {
                    "name": SIMULATOR_METADATA_SCHEMA,
                    "version": SCHEMA_VERSION,
                },
                "executionDuration": u64::try_from(measurements.duration.as_millis())
                    .unwrap_or(u64::MAX),
            },
        },
    })
}

/// Trace a bucket this emulator could not ask Amazon S3 about, which is a deployment with
/// no such service mounted rather than a bucket reported missing.
///
/// The caller's own value is rendered in its escaped `Debug` form, so a name carrying a
/// newline cannot forge a log line.
fn unresolved<E: ::std::error::Error + 'static>(bucket: &str, error: &E) {
    ::tracing::warn!(
        bucket_name = ?bucket,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "a quantum task's output bucket could not be resolved against Amazon S3"
    );
}

/// Trace a result document this emulator could not write, for the same reason and in the
/// same form.
fn unwritten<E: ::std::error::Error + 'static>(bucket: &str, key: &str, error: &E) {
    ::tracing::warn!(
        bucket_name = ?bucket,
        object_key = ?key,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "a quantum task's result document could not be written to Amazon S3"
    );
}

/// Amazon S3's client for `region`, over the in-process callback into this same emulator.
///
/// The tree's ONE construction of that client (`service_common::s3_client`), so this crate
/// configures the hop no differently from every other service that reaches the account's
/// object store.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    ::service_common::s3_client::s3_client(ctx.callback(), region)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The result document reports the run: one measurement row per shot, the measured
    /// qubits they are the values of, and the task's own metadata.
    #[test]
    fn the_result_document_reports_the_run_it_belongs_to() {
        let measurements = TaskMeasurements {
            measured_qubits: vec![0, 1],
            rows: vec![vec![0, 0], vec![1, 1], vec![0, 0]],
            duration: Duration::from_millis(7),
        };
        let action = ::serde_json::json!({
            "braketSchemaHeader": { "name": crate::state::ACTION_OPENQASM, "version": "1" },
            "source": "OPENQASM 3.0; qubit[2] q; h q[0]; cnot q[0], q[1];",
        })
        .to_string();
        let document = gate_model_result(
            ResultContext {
                quantum_task_arn: "arn:aws:braket:eu-west-1:000000000000:quantum-task/abc",
                device_arn: "arn:aws:braket:::device/quantum-simulator/amazon/sv1",
                action: &action,
                shots: 3,
                created_at: "2026-01-01T00:00:00Z",
                ended_at: "2026-01-01T00:00:02Z",
                status: crate::state::STATUS_COMPLETED,
            },
            &measurements,
        );
        assert_eq!(
            document["measurements"],
            ::serde_json::json!([[0, 0], [1, 1], [0, 0]]),
            "case measurements: expected one row per shot, got {}",
            document["measurements"]
        );
        assert_eq!(
            document["measuredQubits"],
            ::serde_json::json!([0, 1]),
            "case measuredQubits: expected the measured qubits, got {}",
            document["measuredQubits"]
        );
        assert_eq!(
            document["taskMetadata"]["shots"],
            ::serde_json::json!(3),
            "case taskMetadata.shots: expected the shot count, got {}",
            document["taskMetadata"]
        );
        assert_eq!(
            document["taskMetadata"]["status"].as_str(),
            Some(crate::state::STATUS_COMPLETED),
            "case taskMetadata.status: expected the task's status, got {}",
            document["taskMetadata"]
        );
        assert_eq!(
            document["additionalMetadata"]["action"]["source"].as_str(),
            Some("OPENQASM 3.0; qubit[2] q; h q[0]; cnot q[0], q[1];"),
            "case additionalMetadata.action: expected the action as sent, got {}",
            document["additionalMetadata"]
        );
        assert_eq!(
            document["additionalMetadata"]["simulatorMetadata"]["executionDuration"],
            ::serde_json::json!(7),
            "case simulatorMetadata: expected the run's own duration, got {}",
            document["additionalMetadata"]
        );
        assert_eq!(
            document["braketSchemaHeader"]["name"].as_str(),
            Some(GATE_MODEL_RESULT_SCHEMA),
            "case schema header: expected the gate-model result schema, got {}",
            document["braketSchemaHeader"]
        );
    }
}
