// SPDX-License-Identifier: CC0-1.0

//! The OFFLINE STORE of a SageMaker Feature Store feature group: where a group's records
//! are written in Amazon S3, under which role, and when that destination is reported as
//! blocked.
//!
//! A feature group's offline store is configured by the SageMaker control plane
//! (`CreateFeatureGroup`'s `OfflineStoreConfig.S3StorageConfig.S3Uri` and the `RoleArn`
//! "used to persist data into the OfflineStore") and WRITTEN by the FeatureStore Runtime
//! data plane, which ingests the records ("If a new record's EventTime is greater, the new
//! record is written to both the OnlineStore and OfflineStore. Otherwise, the record is a
//! historic record and it is written only to the OfflineStore" — `PutRecord`). Two planes,
//! one destination: so the layout of the keys, the question of whether the role may write
//! them and the sentence a blocked destination is explained with live here once, beside the
//! feature-group directory ([`crate::feature_store_groups`]) the same two planes already
//! share. Each plane keeps only its own half — the control plane reports the status, the
//! data plane writes the object and formats its bytes.
//!
//! The key layout is the published one for an offline store (Amazon SageMaker Feature
//! Store, *Offline store*):
//!
//! ```text
//! s3://<bucket>/<prefix>/<account-id>/sagemaker/<region>/offline-store/
//!     <feature-group-name>-<feature-group-creation-time>/data/
//!     year=<YYYY>/month=<MM>/day=<DD>/hour=<HH>/<file>
//! ```
//!
//! Everything down to `data` is fixed when the group is created, which is what
//! `S3StorageConfig$ResolvedOutputS3Uri` ("The S3 path where offline records are written")
//! reports; the partition below it is the record's own EVENT TIME, which is what the
//! offline store is partitioned by.

use std::time::SystemTime;

/// The IAM action writing one object into the offline store is authorized under: "you must
/// have the `s3:PutObject` permission" (Amazon S3 `PutObject`'s own documentation).
pub const WRITE_ACTION: &str = "s3:PutObject";

/// The partition an Amazon S3 ARN carries when the role's own ARN names none this emulator
/// can read.
const DEFAULT_PARTITION: &str = "aws";

/// The service label an Amazon S3 endpoint's host carries.
const S3_HOST_LABEL: &str = "s3";

/// The scheme of the `s3://` spelling of a location, whose authority is the bucket.
const S3_SCHEME: &str = "s3";

/// The scheme of the `https://` spelling, whose authority is an Amazon S3 endpoint.
const HTTPS_SCHEME: &str = "https";

/// What separates a virtual-hosted bucket from the endpoint in a host, where the endpoint
/// spells the Region with a `.` (`<bucket>.s3.<region>.amazonaws.com`).
const HOST_SERVICE_LABEL_DOT: &str = ".s3.";

/// The same, where the endpoint spells the Region with the legacy `-`
/// (`<bucket>.s3-<region>.amazonaws.com`).
const HOST_SERVICE_LABEL_DASH: &str = ".s3-";

/// The longest `BlockedReason` the model carries (`OfflineStoreStatus$BlockedReason`,
/// `length 0..1024`), so an explanation naming a long role ARN and a long location stays
/// inside the member it is reported in.
pub const MAX_BLOCKED_REASON: usize = 1024;

/// `path` with its percent-escapes resolved: a URL path's escaping is the URL's, while what
/// it names here is an Amazon S3 key.
fn decoded(path: &str) -> String {
    ::percent_encoding::percent_decode_str(path)
        .decode_utf8_lossy()
        .into_owned()
}

/// Where one feature group's records are replicated to, as the shared feature-group
/// directory records it for the data plane that writes them.
///
/// The runtime API carries no `OfflineStoreConfig` and no `RoleArn`, so a `PutRecord` can
/// only replicate a record into the offline store because the control plane recorded these
/// two facts when it created the group — the same reason the rest of
/// [`crate::feature_store_groups::FeatureGroupRecord`] is recorded there.
#[derive(Debug, Clone)]
pub struct OfflineStoreDelivery {
    /// `S3StorageConfig$ResolvedOutputS3Uri` — "The S3 path where offline records are
    /// written", i.e. [`resolved_output_s3_uri`] for this group.
    pub resolved_output_s3_uri: String,
    /// The group's `RoleArn`: "The Amazon Resource Name (ARN) of the IAM execution role
    /// used to persist data into the OfflineStore if an OfflineStoreConfig is provided."
    /// Every write is authorized against THIS role's permissions rather than the calling
    /// client's.
    pub role_arn: String,
}

/// An `s3://bucket/key` location of the offline store, split into its parts.
///
/// The ONE reading of a feature group's offline location, for both planes: the control
/// plane asks whether the role may write there, the data plane writes the object itself,
/// and a location read two ways would let one report a destination the other does not use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineStoreLocation {
    /// The bucket the objects are written into.
    pub bucket: String,
    /// The key prefix inside it, with no leading or trailing `/`.
    pub prefix: String,
}

impl OfflineStoreLocation {
    /// The location `s3_uri` names, or `None` for a value that names no bucket.
    ///
    /// `S3Uri`'s modelled pattern is `^(https|s3)://([^/]+)/?(.*)$`, so the member admits
    /// BOTH spellings of one location and each has to reach the same bucket: the `s3://`
    /// form, and the `https://` form of an Amazon S3 endpoint — virtual-hosted
    /// (`https://<bucket>.s3.<region>.amazonaws.com/<prefix>`) or path-style
    /// (`https://s3.<region>.amazonaws.com/<bucket>/<prefix>`). A URI written with a
    /// trailing `/` ("S3 URIs have a format similar to the following:
    /// `s3://example-bucket/prefix/`") names the same prefix as one without, so the
    /// separators are not doubled.
    ///
    /// Only the BUCKET and the key prefix are read out: the host of an `https://` value is
    /// used to recover the bucket name and then dropped, because the write is made to the
    /// emulator's own object store over its in-process transport ([`crate::s3_client`]),
    /// whose address comes from the Region alone. No part of this value is ever a
    /// destination.
    ///
    /// The value is read by the maintained URL parser rather than split by hand — it is a
    /// caller-supplied URI — and its path is percent-DECODED, since that path spells an
    /// Amazon S3 key, whose own character set is wider than a URL path's.
    #[must_use]
    pub fn parse(s3_uri: &str) -> Option<Self> {
        let url = ::url::Url::parse(s3_uri.trim()).ok()?;
        let path = decoded(url.path());
        let host = url.host_str()?;
        match url.scheme() {
            // In an `s3://` URI the bucket IS the authority and the key is the path.
            S3_SCHEME => Self::of(host, &path),
            HTTPS_SCHEME => {
                // Virtual-hosted: the bucket is the label ahead of the service name in the
                // host, whether the endpoint spells the Region with a `.` or the legacy `-`.
                for separator in [HOST_SERVICE_LABEL_DOT, HOST_SERVICE_LABEL_DASH] {
                    if let Some((bucket, _endpoint)) = host.split_once(separator) {
                        return Self::of(bucket, &path);
                    }
                }
                // Path-style: the host is the service endpoint itself and the bucket is the
                // first path segment.
                if host == S3_HOST_LABEL
                    || host.starts_with(&format!("{S3_HOST_LABEL}."))
                    || host.starts_with(&format!("{S3_HOST_LABEL}-"))
                {
                    let trimmed = path.trim_matches('/');
                    let (bucket, prefix) = trimmed.split_once('/').unwrap_or((trimmed, ""));
                    return Self::of(bucket, prefix);
                }
                None
            }
            _ => None,
        }
    }

    /// One location from a bucket and a prefix, and `None` when the bucket is empty: a
    /// value that names no bucket names no location.
    fn of(bucket: &str, prefix: &str) -> Option<Self> {
        (!bucket.is_empty()).then(|| Self {
            bucket: bucket.to_string(),
            prefix: prefix.trim_matches('/').to_string(),
        })
    }

    /// The key `relative` names under this location: the prefix and the tail, joined by a
    /// single separator (a location with no prefix is the bucket's root).
    #[must_use]
    pub fn key(&self, relative: &str) -> String {
        if self.prefix.is_empty() {
            relative.to_string()
        } else {
            format!("{}/{relative}", self.prefix)
        }
    }

    /// This location's URI, as `ResolvedOutputS3Uri` carries one.
    #[must_use]
    pub fn uri(&self) -> String {
        if self.prefix.is_empty() {
            format!("s3://{}", self.bucket)
        } else {
            format!("s3://{}/{}", self.bucket, self.prefix)
        }
    }

    /// The ARN of the object `key` names, which is the resource an Amazon S3 write of it is
    /// authorized against (`arn:<partition>:s3:::<bucket>/<key>` — an Amazon S3 object ARN
    /// carries no Region and no account).
    #[must_use]
    pub fn object_arn(&self, partition: &str, key: &str) -> String {
        format!("arn:{partition}:s3:::{}/{key}", self.bucket)
    }
}

/// `S3StorageConfig$ResolvedOutputS3Uri` for one feature group: "The S3 path where offline
/// records are written."
///
/// The published layout under the `S3Uri` the create named: the account, `sagemaker`, the
/// Region, `offline-store`, the group's own name with the second its creation fell on, and
/// `data`, which is the directory the record objects themselves go under. `None` for an
/// `S3Uri` that names no `s3://` bucket.
///
/// Computed where the group is created, because the creation instant is part of it, and
/// recorded for the data plane ([`OfflineStoreDelivery`]), so the path the control plane
/// REPORTS and the path the data plane WRITES are one string.
#[must_use]
pub fn resolved_output_s3_uri(
    s3_uri: &str,
    account_id: &str,
    region: &str,
    feature_group_name: &str,
    created_at: SystemTime,
) -> Option<String> {
    let location = OfflineStoreLocation::parse(s3_uri)?;
    let created_at_secs = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    Some(format!(
        "{}/{account_id}/sagemaker/{region}/offline-store/\
         {feature_group_name}-{created_at_secs}/{DATA_SEGMENT}",
        location.uri()
    ))
}

/// The directory of the resolved path the record objects themselves are written under.
const DATA_SEGMENT: &str = "data";

/// The key of ONE record object under the resolved output path `location`: the event-time
/// partition and the file's own name.
///
/// The offline store is partitioned by the record's EVENT TIME — `year=<YYYY>/month=<MM>/
/// day=<DD>/hour=<HH>`, the partition the published layout (and the Glue table over it)
/// names — so a record of an hour lands in that hour's partition whenever it is ingested,
/// which is what makes a backfilled historic record readable beside the records of its own
/// hour. `event_time` is the instant the ingested record's event-time feature carried; an
/// ingestion whose record carries none is partitioned by the instant it was ingested at,
/// since that is the only instant it has.
///
/// The file's name carries the ingestion instant and the unique field every delivered key
/// of this crate ends with ([`crate::trail_log_files::unique_field`]), so two records
/// written into one partition are two objects rather than one overwriting the other.
#[must_use]
pub fn record_object_key(
    location: &OfflineStoreLocation,
    event_time: SystemTime,
    written_at: SystemTime,
    suffix: &str,
) -> String {
    let partition = calendar(event_time)
        .or_else(|| calendar(written_at))
        .unwrap_or_default()
        .format("year=%Y/month=%m/day=%d/hour=%H");
    let written = calendar(written_at)
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%SZ");
    location.key(&format!(
        "{partition}/{written}_{}{suffix}",
        crate::trail_log_files::unique_field()
    ))
}

/// `at` as the UTC calendar fields a key is laid out from, and `None` for an instant
/// outside the range a calendar date can state.
///
/// Read through the maintained calendar crate's FALLIBLE reading of an instant, so an event
/// time far outside the dates a timestamp can name — a value a request may carry, since an
/// event-time feature may be any number of seconds since the epoch — yields no calendar
/// rather than failing the call it arrived on.
fn calendar(at: SystemTime) -> Option<::chrono::DateTime<::chrono::Utc>> {
    let seconds = match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).ok()?,
        Err(before) => i64::try_from(before.duration().as_secs())
            .ok()
            .and_then(i64::checked_neg)?,
    };
    ::chrono::DateTime::from_timestamp(seconds, 0)
}

/// Whether IAM ANSWERED that the role `role_arn` may NOT write the object `key` of
/// `location`.
///
/// The write is made UNDER the feature group's own role — "the IAM execution role used to
/// persist data into the OfflineStore" — so it is that role's permissions that decide it,
/// not the calling client's. The question is put to IAM itself, through the tree's one
/// route to that answer ([`crate::iam_role::role_withholds_action_through`]), which is why
/// everything that is not such an answer (a value naming no role of this account, a role
/// this emulator's IAM holds nothing under, no IAM to ask at all) withholds nothing: an
/// execution role is routinely provisioned out of band, so its absence here is evidence
/// about this store and not about the role's permissions.
pub async fn write_is_withheld(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    role_arn: &str,
    location: &OfflineStoreLocation,
    key: &str,
) -> bool {
    let partition = crate::arn::parse(role_arn).map_or(DEFAULT_PARTITION, |parsed| parsed.partition);
    let object_arn = location.object_arn(partition, key);
    crate::iam_role::role_withholds_action_through(
        callback,
        region,
        role_arn,
        WRITE_ACTION,
        &object_arn,
    )
    .await
}

/// The key the STATUS of an offline store is decided for: every object a record ingestion
/// writes is under the resolved output path, so the permission in force over that path is
/// what the status reports.
#[must_use]
pub fn status_key(location: &OfflineStoreLocation) -> String {
    location.key("*")
}

/// How a blocked offline store is explained: "The justification for why the
/// OfflineStoreStatus is Blocked (if applicable)" (`OfflineStoreStatus$BlockedReason`).
///
/// It names the three facts a client fixes the block with — the role the replication runs
/// as, the permission it is missing and the location it is missing it on — and is cut to
/// the modelled [`MAX_BLOCKED_REASON`] at a character boundary rather than reported outside
/// the member's bound.
#[must_use]
pub fn blocked_reason(role_arn: &str, resolved_output_s3_uri: &str) -> String {
    let mut reason = format!(
        "The IAM role {role_arn} does not have permission to perform {WRITE_ACTION} on the \
         OfflineStore location {resolved_output_s3_uri}. Grant the role that permission for \
         records to be replicated into the OfflineStore."
    );
    if reason.len() > MAX_BLOCKED_REASON {
        let cut = (0..=MAX_BLOCKED_REASON)
            .rev()
            .find(|at| reason.is_char_boundary(*at))
            .unwrap_or_default();
        reason.truncate(cut);
    }
    reason
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// An instant with a known UTC calendar rendering: 2026-06-01T07:08:09Z.
    fn at(rfc3339: &str) -> SystemTime {
        ::chrono::DateTime::parse_from_rfc3339(rfc3339)
            .expect("the fixture instant is a well-formed timestamp")
            .into()
    }

    /// RULE: a location is the bucket and the key prefix an `S3Uri` names, in EITHER
    /// spelling the member's pattern admits, with its separators not doubled; a value
    /// naming no bucket is no location at all.
    ///
    /// The rows differ along the prefix's shape (none, one segment, several, a trailing
    /// delimiter) and the spelling of the URI (`s3://`, the virtual-hosted `https://` form
    /// with each Region separator, the path-style `https://` form, and a value of another
    /// scheme).
    #[test]
    fn a_location_is_the_bucket_and_prefix_an_s3_uri_names() {
        type Row = (&'static str, &'static str, Option<(&'static str, &'static str)>);

        // (case, the URI, the bucket and prefix it names)
        let rows: [Row; 13] = [
            ("a bucket and a prefix", "s3://bucket/offline", Some(("bucket", "offline"))),
            (
                "a nested prefix",
                "s3://bucket/offline/store",
                Some(("bucket", "offline/store")),
            ),
            ("a trailing delimiter", "s3://bucket/offline/", Some(("bucket", "offline"))),
            ("a bucket alone", "s3://bucket", Some(("bucket", ""))),
            ("a bucket and an empty prefix", "s3://bucket/", Some(("bucket", ""))),
            (
                "the virtual-hosted https spelling",
                "https://bucket.s3.amazonaws.com/offline",
                Some(("bucket", "offline")),
            ),
            (
                "the virtual-hosted https spelling of a Region's endpoint",
                "https://bucket.s3.eu-west-1.amazonaws.com/offline/store",
                Some(("bucket", "offline/store")),
            ),
            (
                "the legacy dash-Region virtual-hosted spelling",
                "https://bucket.s3-eu-west-1.amazonaws.com/offline",
                Some(("bucket", "offline")),
            ),
            (
                "the path-style https spelling",
                "https://s3.eu-west-1.amazonaws.com/bucket/offline",
                Some(("bucket", "offline")),
            ),
            (
                "the path-style https spelling of a bucket alone",
                "https://s3.amazonaws.com/bucket",
                Some(("bucket", "")),
            ),
            ("another scheme", "ftp://bucket/offline", None),
            ("no bucket at all", "s3:///offline", None),
            (
                "a prefix carrying a character a URL path escapes",
                "s3://bucket/offline store",
                Some(("bucket", "offline store")),
            ),
        ];
        for (case, uri, expected) in rows {
            let parsed = OfflineStoreLocation::parse(uri);
            let read = parsed
                .as_ref()
                .map(|location| (location.bucket.as_str(), location.prefix.as_str()));
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: the resolved output path is the published layout under the configured `S3Uri`
    /// — the account, `sagemaker`, the Region, `offline-store`, the group and the second its
    /// creation fell on, then `data`.
    #[test]
    fn the_resolved_output_path_carries_the_published_layout() {
        // (case, the configured S3Uri, the path it resolves to)
        let rows = [
            (
                "a bucket and a prefix",
                "s3://features/offline",
                Some(
                    "s3://features/offline/123456789012/sagemaker/eu-west-1/offline-store/\
                     customers-1780000000/data"
                        .to_string(),
                ),
            ),
            (
                "a URI written with a trailing delimiter",
                "s3://features/offline/",
                Some(
                    "s3://features/offline/123456789012/sagemaker/eu-west-1/offline-store/\
                     customers-1780000000/data"
                        .to_string(),
                ),
            ),
            (
                "a bucket alone",
                "s3://features",
                Some(
                    "s3://features/123456789012/sagemaker/eu-west-1/offline-store/\
                     customers-1780000000/data"
                        .to_string(),
                ),
            ),
            (
                "the https spelling of a location",
                "https://features.s3.eu-west-1.amazonaws.com/offline",
                Some(
                    "s3://features/offline/123456789012/sagemaker/eu-west-1/offline-store/\
                     customers-1780000000/data"
                        .to_string(),
                ),
            ),
            ("a value that is no s3 URI", "features/offline", None),
        ];
        let created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_780_000_000);
        for (case, s3_uri, expected) in rows {
            let resolved = resolved_output_s3_uri(
                s3_uri,
                "123456789012",
                "eu-west-1",
                "customers",
                created_at,
            );
            assert_eq!(
                resolved, expected,
                "case {case:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    /// RULE: a record object's key is the event-time partition under the resolved output
    /// path, and two records of one partition are two objects.
    #[test]
    fn a_record_object_is_keyed_by_its_event_time_partition() {
        let location = OfflineStoreLocation::parse(
            "s3://features/offline/123456789012/sagemaker/eu-west-1/offline-store/\
             customers-1780000000/data",
        )
        .expect("the resolved path names a location");
        let written_at = at("2026-09-02T10:23:43Z");
        // (case, the record's event time, the partition its key must carry)
        let rows = [
            ("an event time of its own", "2026-06-01T07:08:09Z", "year=2026/month=06/day=01/hour=07"),
            ("an event time at the turn of a year", "2027-01-01T00:00:00Z", "year=2027/month=01/day=01/hour=00"),
            ("an event time in the last hour of a day", "2026-02-28T23:59:59Z", "year=2026/month=02/day=28/hour=23"),
        ];
        for (case, event_time, partition) in rows {
            let key = record_object_key(&location, at(event_time), written_at, ".parquet");
            let expected_prefix = format!("{}/{partition}/", location.prefix);
            assert!(
                key.starts_with(&expected_prefix),
                "case {case:?}: expected a key under {expected_prefix:?}, got {key:?}"
            );
            assert!(
                key.ends_with(".parquet") && key.contains("20260902T102343Z_"),
                "case {case:?}: expected the written instant and the suffix in the name, got \
                 {key:?}"
            );
            let second = record_object_key(&location, at(event_time), written_at, ".parquet");
            assert_ne!(
                key, second,
                "case {case:?}: expected two ingestions to be two objects, got {key:?} twice"
            );
        }
    }

    /// RULE: the blocked reason names the role, the permission and the location, and stays
    /// inside the member's modelled bound whatever it names.
    #[test]
    fn the_blocked_reason_names_the_role_the_action_and_the_location() {
        let role = "arn:aws:iam::123456789012:role/offline-store";
        let reason = blocked_reason(role, "s3://features/offline/data");
        for expected in [role, WRITE_ACTION, "s3://features/offline/data"] {
            assert!(
                reason.contains(expected),
                "case a blocked offline store: expected {expected:?} in the reason, got \
                 {reason:?}"
            );
        }
        let long_role = format!("arn:aws:iam::123456789012:role/{}", "r".repeat(2_000));
        let bounded = blocked_reason(&long_role, "s3://features/offline/data");
        assert!(
            bounded.len() <= MAX_BLOCKED_REASON,
            "case a long role ARN: expected at most {MAX_BLOCKED_REASON} bytes, got {}",
            bounded.len()
        );
    }
}
