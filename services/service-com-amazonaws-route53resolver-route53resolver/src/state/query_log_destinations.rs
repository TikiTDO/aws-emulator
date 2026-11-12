// SPDX-License-Identifier: CC0-1.0

//! The log destination a query logging configuration delivers to, resolved
//! against the inventory of the service that OWNS it.
//!
//! `CreateResolverQueryLogConfig$DestinationArn` is "the ARN of the resource
//! that you want Resolver to send query logs", and the resources Resolver
//! delivers to — an Amazon S3 bucket, a CloudWatch Logs log group and a Kinesis
//! Data Firehose delivery stream ([`QUERY_LOG_DESTINATIONS`]) — are resources of
//! OTHER services of the account. Which of them the account holds is a fact of
//! those services' own stores, read here through their own accessors, exactly as
//! [`unresolvable_vpc_id`] resolves a VPC id against the account's Amazon EC2
//! inventory and [`firewall_domain_file_at`] reads an object out of its Amazon
//! S3 inventory.

use super::*;

/// Whether the account holds the destination `destination_arn` names.
///
/// A query logging configuration delivers to a destination that EXISTS: one
/// addressing a resource the account does not hold could never deliver a log
/// line, which is why the create that names it is refused rather than stored. An
/// ARN that names no kind of destination Resolver delivers to is not one of them
/// either — [`query_log_destination_is_wellformed`] is what tells the caller
/// that, so this answers `false` for it too and the handler checks the form
/// first.
///
/// A pure read of the owning service's store: nothing is written to it.
///
/// LOCK ORDER. As [`firewall_domain_file_at`]: the owning store's lock is taken
/// for this one pass and released before returning, and no guard of this
/// service's own store is held while it is — which is what keeps a lock cycle
/// between two stores unreachable.
pub fn account_holds_query_log_destination<Eng: Send + Sync + 'static>(
    engine: &Eng,
    destination_arn: &str,
) -> bool {
    let fields: Vec<&str> = destination_arn.splitn(6, ':').collect();
    let [_prefix, _partition, service, region, _account, resource] = fields.as_slice() else {
        return false;
    };
    match *service {
        // "arn:aws:s3:::amzn-s3-demo-bucket", optionally with the key prefix
        // Resolver writes its logs under: the destination is the BUCKET, which
        // is the first path segment of the resource part.
        "s3" => {
            let bucket = resource.split('/').next().unwrap_or_default();
            !bucket.is_empty() && account_holds_bucket(engine, bucket)
        }
        // "arn:aws:logs:<region>:<account>:log-group:<name>[:*]": the trailing
        // log-stream wildcard names every stream of the group rather than part
        // of the group's identity ([`canonical_query_log_destination`]), so the
        // group is what is resolved.
        "logs" => {
            let group = resource
                .strip_prefix("log-group:")
                .map(|group| group.strip_suffix(":*").unwrap_or(group))
                .unwrap_or_default();
            !group.is_empty() && account_holds_log_group(engine, group)
        }
        // "arn:aws:kinesis:<region>:<account>:stream/<name>", held by the
        // Region's own endpoint: a Kinesis stream name "is scoped to the Amazon
        // Web Services account used by the application and to the Region", so
        // the Region the ARN names is part of the lookup.
        "kinesis" => {
            let stream = resource.strip_prefix("stream/").unwrap_or_default();
            !stream.is_empty() && account_holds_kinesis_stream(engine, region, stream)
        }
        // "arn:aws:firehose:<region>:<account>:deliverystream/<name>".
        "firehose" => {
            let stream = resource
                .strip_prefix("deliverystream/")
                .unwrap_or_default();
            !stream.is_empty() && account_holds_delivery_stream(engine, stream)
        }
        _ => false,
    }
}

/// Whether the account's own Amazon S3 inventory holds the bucket `bucket`.
fn account_holds_bucket<Eng: Send + Sync + 'static>(engine: &Eng, bucket: &str) -> bool {
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        // No store for that service on this engine is no inventory to resolve
        // against, so no destination can be shown to name nothing — the same
        // allowance [`unresolvable_ec2_id`] makes.
        return true;
    };
    let held = state.lock().bucket(bucket).is_some();
    held
}

/// Whether the account's own CloudWatch Logs inventory holds the log group
/// `group`.
fn account_holds_log_group<Eng: Send + Sync + 'static>(engine: &Eng, group: &str) -> bool {
    let Some(state) =
        ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state::service_state(engine)
    else {
        return true;
    };
    let held = state.lock().log_groups.contains_key(group);
    held
}

/// Whether the account's own Amazon Kinesis Data Streams inventory holds the
/// stream `stream` in `region`.
fn account_holds_kinesis_stream<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    stream: &str,
) -> bool {
    let Some(state) = ::service_com_amazonaws_kinesis_kinesis_20131202::state::service_state(engine)
    else {
        return true;
    };
    let held = state
        .lock()
        .stream_in(SystemTime::now(), region, stream)
        .is_some();
    held
}

/// Whether the account's own Kinesis Data Firehose inventory holds the delivery
/// stream `stream`.
fn account_holds_delivery_stream<Eng: Send + Sync + 'static>(engine: &Eng, stream: &str) -> bool {
    let Some(state) =
        ::service_com_amazonaws_firehose_firehose_20150804::state::service_state(engine)
    else {
        return true;
    };
    let held = state.lock().streams.contains_key(stream);
    held
}
