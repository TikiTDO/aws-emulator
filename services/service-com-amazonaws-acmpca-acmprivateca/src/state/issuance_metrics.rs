// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch metric series a private CA publishes about the issuances it serves.
//!
//! Amazon Web Services Private CA reports its issuance work twice. Once inline, as the
//! certificate ARN `IssueCertificate` answers with and the certificate `GetCertificate`
//! then serves, and once as metric data in the `AWS/ACMPrivateCA` namespace, which the
//! service's monitoring documentation describes with three metrics of one event — an
//! issuance request:
//!
//! * `Success` — the issuances that succeeded, which applies only to `IssueCertificate`;
//! * `Failure` — the issuance attempts that failed;
//! * `Time` — the time in milliseconds between an issuance request and the completion or
//!   failure of issuance.
//!
//! So one issuance request contributes one `Time` observation and one observation of its
//! outcome: a request the CA completes is a `Success`, one that fails after the CA has taken
//! it on is a `Failure`. Each series belongs to the certificate authority that served the
//! request, so it is dimensioned on that CA: a CA that served no issuance has no series at
//! all rather than a share of an aggregate, which is what lets a client alarm on, or graph,
//! ONE private CA's issuance.
//!
//! The datapoints go into the CloudWatch emulator's own metric store — the SAME store
//! `PutMetricData` writes and `ListMetrics` / `GetMetricStatistics` read — reached through
//! the shared engine, so one metric has one home and the publishers cannot drift.
//!
//! What publishes, and what does not:
//!
//! * A request the CA completes — the one that issues a certificate, and the `@idempotent`
//!   repeat that is answered with the certificate the first one issued — is an issuance that
//!   succeeded, so it contributes one `Success` observation and the `Time` it took.
//! * A request the service REFUSES before the CA takes it on — an unusable argument, a
//!   malformed certification request, a CA in the wrong state, a validity the CA's own
//!   certificate does not cover — publishes nothing at all, so a CA whose issuance requests
//!   were refused has no `Failure` series rather than a zero one. `Failure` carries an
//!   observation for an issuance that fails once the CA has taken it on, during signing;
//!   this service signs inside the call that asks for it, so every failure it has is a
//!   refusal made before that point and no datapoint of that series exists.
//! * The certificate a CA signs while it is still waiting for its own — the self-signed root
//!   CA certificate its activation imports — is accounted to no issuer: it is what gives the
//!   CA the certificate every other issuance is served under, so the CA starts accounting
//!   for the issuances it serves once it holds it. In the recorded run three issuance
//!   requests reached one CA, the first of them that self-signed certificate, and the CA's
//!   `Success` series summed to the two served afterwards.

use std::time::Duration;

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

/// The namespace Amazon Web Services Private CA publishes its metrics into.
pub const ACMPCA_NAMESPACE: &str = "AWS/ACMPrivateCA";

/// The dimension every series of that namespace carries: the certificate authority the
/// series accounts for, named by its ARN — the identifier this service's own API names a CA
/// by (`IssueCertificateRequest$CertificateAuthorityArn`), and the one a client already
/// holds for the CA it is monitoring.
pub const CERTIFICATE_AUTHORITY_DIMENSION: &str = "CertificateAuthorityArn";

/// The metric counting the issuances a CA completed.
pub const SUCCESS_METRIC: &str = "Success";

/// The metric counting the issuances that failed after the CA took them on.
pub const FAILURE_METRIC: &str = "Failure";

/// The metric carrying how long an issuance took.
pub const TIME_METRIC: &str = "Time";

/// The unit a count of issuances is observed in: a count is neither a time nor a size.
pub const COUNT_UNIT: &str = "Count";

/// The unit an issuance latency is observed in, as the metric is documented in
/// milliseconds.
pub const MILLISECONDS_UNIT: &str = "Milliseconds";

/// The issuances one completed request is: a request asks for one certificate, so it
/// contributes exactly one observation to the `Success` series.
const ONE_ISSUANCE: f64 = 1.0;

/// The resolution these series are stored at, in seconds — the regular resolution a metric
/// has unless its publisher asks for the high-resolution one ("This field is optional, if
/// you do not specify it the default of 60 is used", `MetricDatum$StorageResolution`), which
/// is the one-minute aggregation these metrics are published at.
pub const STORAGE_RESOLUTION_SECS: i32 = 60;

/// Publish what ONE issuance the CA `certificate_authority_arn` completed contributes to the
/// `AWS/ACMPrivateCA` namespace: one `Success` observation and one `Time` observation of
/// `took`, the time that issuance took.
///
/// `took` is reported in milliseconds, the unit the metric is documented in, keeping the
/// fraction of a millisecond a measurement shorter than one carries: the observation is how
/// long the issuance took, so it is never rounded away to no time at all.
///
/// Nothing is published when the CloudWatch store is not mounted beside this service: a
/// datapoint has one home and this path never makes a second one. Call it with no private CA
/// store guard held, so the two services' stores are never locked at once. A store at its
/// series capacity takes no new series; the issuance it came from is not failed for that,
/// because the certificate was already served.
pub fn publish_completed_issuance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    certificate_authority_arn: &str,
    took: Duration,
) {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    // CloudWatch holds a datapoint's instant as epoch SECONDS, which is the unit its own
    // readers window by; the issuance has just completed, so that instant is now.
    let timestamp_secs = cloudwatch::state::now_secs();
    let dimensions: cloudwatch::state::DimensionSet = [(
        CERTIFICATE_AUTHORITY_DIMENSION.to_string(),
        certificate_authority_arn.to_string(),
    )]
    .into_iter()
    .collect();
    let mut store = state.lock();
    for (metric_name, value, unit) in [
        (SUCCESS_METRIC, ONE_ISSUANCE, COUNT_UNIT),
        (
            TIME_METRIC,
            took.as_secs_f64() * 1_000.0,
            MILLISECONDS_UNIT,
        ),
    ] {
        let key = cloudwatch::state::MetricKey {
            namespace: ACMPCA_NAMESPACE.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: dimensions.clone(),
        };
        let datum = cloudwatch::state::StoredDatum {
            timestamp_secs,
            // One observation: this issuance's own contribution to the series.
            sample_count: 1.0,
            sum: value,
            minimum: value,
            maximum: value,
            unit: Some(unit.to_string()),
            storage_resolution: STORAGE_RESOLUTION_SECS,
        };
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series; the issuance it came from was already
            // served, so the call is not failed for it. The dropped observation is reported
            // where a developer can see it.
            ::tracing::warn!(
                metric = metric_name,
                certificate_authority = certificate_authority_arn,
                "the CloudWatch metric store is at capacity, so this issuance was not counted"
            );
        }
    }
}
