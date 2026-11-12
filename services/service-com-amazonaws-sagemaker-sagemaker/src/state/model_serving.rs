// SPDX-License-Identifier: CC0-1.0

//! Serving an INFERENCE from a model this emulator's own training produced: the variant an
//! invocation routes to, the artifact behind it, and the prediction the fitted model computes
//! for the payload the caller sent.
//!
//! # What an endpoint DOES
//!
//! "After you deploy a model into production using Amazon SageMaker AI hosting services, your
//! client applications use this API to get inferences from the model hosted at the specified
//! endpoint" (`InvokeEndpoint`). So the endpoint's work is: find the model behind the variant
//! the request routes to, load that model's artifacts, and answer what the model computes for
//! the payload.
//!
//! This module performs it IN PROCESS, over state this emulator really holds:
//!
//! * the variant is resolved through the endpoint's own endpoint configuration — the one
//!   `CreateEndpoint` snapshotted — so the model behind it is the model the caller deployed;
//! * that model's `ModelDataUrl` is read out of this emulator's own Amazon S3 and the fitted
//!   parameters are read back out of the `model.tar.gz` a training or AutoML run wrote
//!   ([`super::model_from_archive`]), so the model served is the model trained;
//! * the payload is read as the caller's own records and the prediction is COMPUTED from
//!   those parameters.
//!
//! Nothing here runs a container image. An artifact this emulator's own runs did not produce
//! holds no parameters to compute from, so there is nothing to serve for it and the caller
//! gets the operation's modelled failure for an invocation the model did not answer — never a
//! synthesised body. The same goes for a variant whose model carries no artifact at all, and
//! for a payload holding no record the model can be applied to.

use super::*;

use crate::ops::types::ContainerDefinition;

/// The most records one invocation's payload may carry. An EMULATOR ceiling, not an AWS
/// limit: the predictions are held in memory, so the answer is bounded however large the
/// payload the modelled `Body` length admits.
pub const MAX_PAYLOAD_RECORDS: usize = 1_000;

/// The most feature values one record contributes, which is the width the records of a
/// payload are read against. An EMULATOR ceiling, not an AWS limit, so a record is bounded
/// whatever the payload carries; it matches the width the fits here are bounded to
/// ([`super::MAX_FEATURES`]).
pub const MAX_PAYLOAD_FEATURES: usize = super::MAX_FEATURES;

/// The media type a payload of comma-separated values carries, and the one the answer is
/// rendered in unless the request asks for another ("text/csv" is the content type SageMaker's
/// built-in algorithms take and answer records in).
pub const CSV_MEDIA_TYPE: &str = "text/csv";

/// The media type an answer is rendered as JSON for, when the request's `Accept` asks for it.
pub const JSON_MEDIA_TYPE: &str = "application/json";

/// What serves an invocation behind one of an endpoint's production variants.
#[derive(Debug, Clone)]
pub struct ServingVariant {
    /// `ProductionVariant$VariantName`, which is what `TargetVariant` names.
    pub variant_name: String,
    /// `ProductionVariant$ModelName`: the model deployed behind the variant.
    pub model_name: String,
    /// The model's primary container's `ModelDataUrl`, when it names one.
    pub model_data_url: Option<String>,
}

/// Why nothing could serve an invocation: the condition that stopped it, as the modelled
/// failure for an invocation the model did not answer reports it.
#[derive(Debug, Clone)]
pub enum ServingRefusal {
    /// The variant's model carries no model artifact, so there are no parameters to serve.
    NoModelArtifact { model_name: String },
    /// The artifact could not be read out of Amazon S3.
    ArtifactUnreadable { uri: String },
    /// The artifact holds no model this emulator can compute an inference from — it was not
    /// produced by a run of this emulator's own, and nothing here runs the container image
    /// that would know how to read it.
    ArtifactNotServable { uri: String },
    /// The payload holds no record the model can be applied to.
    PayloadNotRecords,
    /// The payload carries more records than one invocation may.
    PayloadTooManyRecords { records: usize },
    /// The payload's records carry a different number of features than the model was fitted
    /// on, so the model cannot be applied to them.
    FeatureCountMismatch { expected: usize, found: usize },
}

impl ServingRefusal {
    /// The sentence this condition is reported with, naming what stopped the inference.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NoModelArtifact { model_name } => format!(
                "Model {model_name} carries no ModelDataUrl, so the endpoint has no model \
                 artifacts to serve an inference from."
            ),
            Self::ArtifactUnreadable { uri } => format!(
                "The model artifacts at {uri} could not be read, so no inference was computed."
            ),
            Self::ArtifactNotServable { uri } => format!(
                "The model artifacts at {uri} hold no model this emulator can compute an \
                 inference from: a SageMaker model runs as a container image, which this \
                 emulator does not run, so only an artifact written by its own training runs \
                 can be served."
            ),
            Self::PayloadNotRecords => format!(
                "The request payload holds no record to predict from: a record is a row of \
                 numeric feature values, as {CSV_MEDIA_TYPE} or as a JSON array of them."
            ),
            Self::PayloadTooManyRecords { records } => format!(
                "The request payload holds {records} records, past the {MAX_PAYLOAD_RECORDS} \
                 one invocation may carry."
            ),
            Self::FeatureCountMismatch { expected, found } => format!(
                "The request payload's records carry {found} feature values, and the model \
                 behind this endpoint was fitted on {expected}."
            ),
        }
    }
}

/// The variants `endpoint_name` serves, in the order its endpoint configuration listed them,
/// each with the model deployed behind it and that model's artifact.
///
/// Read through the endpoint's OWN configuration — the one `CreateEndpoint` resolved — so the
/// model behind a variant is the model the caller deployed rather than whatever a later
/// configuration of the same name says.
#[must_use]
pub fn endpoint_serving_variants(
    store: &ComAmazonawsSagemakerSagemakerState,
    endpoint_name: &str,
) -> Vec<ServingVariant> {
    let Some(endpoint) = store.endpoints.get(endpoint_name) else {
        return Vec::new();
    };
    let Some(config) = store.endpoint_configs.get(&endpoint.config_name) else {
        return Vec::new();
    };
    config
        .production_variants
        .iter()
        // `ProductionVariant$ModelName` is optional in the model (a variant may carry an
        // inference component instead), and a variant naming no model has nothing deployed
        // behind it to serve from, so it is not a serving variant at all.
        .filter_map(|variant| {
            let model_name = variant.model_name.clone()?;
            Some(ServingVariant {
                variant_name: variant.variant_name.clone(),
                model_data_url: store.models.get(&model_name).and_then(model_artifact_url),
                model_name,
            })
        })
        .collect()
}

/// Where a model's artifacts are: its primary container's `ModelDataUrl`, and otherwise the
/// first of its inference containers that names one.
///
/// "PrimaryContainer - The location of the primary docker image containing inference code"
/// and `Containers` carries the pipeline's containers; either way the artifact a prediction is
/// computed from is the `ModelDataUrl` the container names.
fn model_artifact_url(model: &Model) -> Option<String> {
    let containers = model
        .primary_container
        .iter()
        .chain(model.containers.iter().flatten());
    containers.filter_map(container_artifact_url).next()
}

/// One container's `ModelDataUrl`, when it names one.
fn container_artifact_url(container: &ContainerDefinition) -> Option<String> {
    container
        .model_data_url
        .as_ref()
        .filter(|uri| !uri.is_empty())
        .cloned()
}

/// Load the fitted model behind `variant`, reading its artifact out of this emulator's own
/// Amazon S3.
pub async fn load_served_model(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    variant: &ServingVariant,
) -> Result<LinearModel, ServingRefusal> {
    let Some(uri) = variant.model_data_url.as_deref() else {
        return Err(ServingRefusal::NoModelArtifact {
            model_name: variant.model_name.clone(),
        });
    };
    let Some(location) = S3Location::parse(uri) else {
        return Err(ServingRefusal::ArtifactUnreadable {
            uri: uri.to_string(),
        });
    };
    let client = s3_client(ctx, region);
    let Ok(bytes) = read_object(&client, &location, MAX_OBJECT_BYTES).await else {
        return Err(ServingRefusal::ArtifactUnreadable {
            uri: location.uri(),
        });
    };
    model_from_archive(&bytes).ok_or(ServingRefusal::ArtifactNotServable {
        uri: location.uri(),
    })
}

/// What an endpoint answers one invocation with: the rendered body and the media type it is
/// rendered in.
#[derive(Debug, Clone)]
pub struct ServedInference {
    pub body: Vec<u8>,
    pub content_type: String,
}

/// Apply `model` to the records `body` carries and render the predictions in the media type
/// `accept` asks for.
pub fn serve_inference(
    model: &LinearModel,
    body: &[u8],
    accept: Option<&str>,
) -> Result<ServedInference, ServingRefusal> {
    let rows = payload_records(body)?;
    if rows.is_empty() {
        return Err(ServingRefusal::PayloadNotRecords);
    }
    if rows.len() > MAX_PAYLOAD_RECORDS {
        return Err(ServingRefusal::PayloadTooManyRecords { records: rows.len() });
    }
    let expected = model.weights.len();
    if let Some(found) = rows.iter().find(|row| row.len() != expected).map(Vec::len) {
        return Err(ServingRefusal::FeatureCountMismatch { expected, found });
    }
    let predictions: Vec<f64> = rows.iter().map(|row| model.predict(row)).collect();
    let as_json = accept.is_some_and(|accept| accept.contains(JSON_MEDIA_TYPE));
    if as_json {
        let document = ::serde_json::json!({
            "predictions": predictions
                .iter()
                .map(|score| ::serde_json::json!({ "score": score }))
                .collect::<Vec<_>>(),
        });
        return Ok(ServedInference {
            body: ::serde_json::to_vec(&document).unwrap_or_default(),
            content_type: JSON_MEDIA_TYPE.to_string(),
        });
    }
    // One record in, one prediction out, in the order the payload carried them — the form
    // SageMaker's built-in algorithms answer a `text/csv` payload in.
    let rendered = predictions
        .iter()
        .map(f64::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ServedInference {
        body: rendered.into_bytes(),
        content_type: CSV_MEDIA_TYPE.to_string(),
    })
}

/// The records one invocation's payload carries: a row of numeric feature values each.
///
/// A payload that opens as a JSON object or array is read as JSON — an `instances` list, a
/// list of rows, or one row — and anything else as comma-separated values, which is the form
/// a `text/csv` payload carries. A row that holds no number at all is not a record and is
/// skipped; a payload holding no record at all is refused.
fn payload_records(body: &[u8]) -> Result<Vec<Vec<f64>>, ServingRefusal> {
    let text = ::std::str::from_utf8(body).map_err(|_| ServingRefusal::PayloadNotRecords)?;
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(trimmed) {
            return Ok(json_records(&document));
        }
    }
    Ok(csv_records(text))
}

/// The records a JSON payload carries: the rows of its `instances` list when it has one, the
/// rows of a bare list, or the single row a bare list of numbers is.
fn json_records(document: &::serde_json::Value) -> Vec<Vec<f64>> {
    let rows = document
        .get("instances")
        .or_else(|| document.get("inputs"))
        .unwrap_or(document);
    let Some(items) = rows.as_array() else {
        return Vec::new();
    };
    // A list whose every element is a number is ONE record, not a list of them.
    if items.iter().all(::serde_json::Value::is_number) {
        return numbers(items).map(|row| vec![row]).unwrap_or_default();
    }    items
        .iter()
        .take(MAX_PAYLOAD_RECORDS + 1)
        .filter_map(json_record)
        .collect()
}

/// One record of a JSON payload: a list of numbers, or an object carrying one under
/// `features`.
fn json_record(item: &::serde_json::Value) -> Option<Vec<f64>> {
    let values = match item {
        ::serde_json::Value::Array(values) => values.as_slice(),
        ::serde_json::Value::Object(_) => item.get("features")?.as_array()?.as_slice(),
        _ => return None,
    };
    numbers(values)
}

/// `values` as a row of numbers, and `None` when any of the first
/// [`MAX_PAYLOAD_FEATURES`] of them is not one. A record wider than that is read against its
/// first [`MAX_PAYLOAD_FEATURES`] values, so one record's width is bounded however long the
/// list is.
fn numbers(values: &[::serde_json::Value]) -> Option<Vec<f64>> {
    values
        .iter()
        .take(MAX_PAYLOAD_FEATURES)
        .map(::serde_json::Value::as_f64)
        .collect::<Option<Vec<f64>>>()
}

/// The records a comma-separated payload carries, read by the maintained CSV reader rather
/// than by splitting text here.
fn csv_records(text: &str) -> Vec<Vec<f64>> {
    let mut reader = ::csv::ReaderBuilder::new()
        // "SageMaker requires that a CSV file doesn't have a header record", so every row is
        // a record.
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut rows: Vec<Vec<f64>> = Vec::new();
    for row in reader.records() {
        if rows.len() > MAX_PAYLOAD_RECORDS {
            return rows;
        }
        let Ok(row) = row else { continue };
        let values: Option<Vec<f64>> = row
            .iter()
            .take(MAX_PAYLOAD_FEATURES)
            .map(|field| field.trim().parse::<f64>().ok())
            .collect();
        if let Some(values) = values.filter(|values| !values.is_empty()) {
            rows.push(values);
        }
    }
    rows
}
