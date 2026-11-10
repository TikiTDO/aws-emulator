// SPDX-License-Identifier: CC0-1.0

//! @op DescribeClusterVersions
//! IMPLEMENTED. Reports the Kubernetes version calendar this service's state holds
//! (`crate::state::ClusterVersionRecord`, seeded from the published Amazon EKS
//! release calendar, newest version first), filtered by the request and paged with
//! an opaque `nextToken` derived from the last calendar row returned.
//!
//! This is a REPORTING operation: it writes nothing. A row's support status is not
//! stored but DERIVED per request from the clock and the row's two end-of-support
//! dates, so the same calendar keeps answering correctly as time passes. The rest
//! of a row — its platform version, patch version, and the provisioned
//! control-plane scaling tiers and component-config defaults and constraints the
//! service publishes — comes from state too (`crate::state`), so every member of
//! a reported `ClusterVersionInformation` has one source.
//!
//! What each input member does here:
//!
//! * `clusterVersions` — keeps only the named versions ("List of specific cluster
//!   versions to describe"); a version the calendar does not publish simply does
//!   not appear. The filter NAMES Kubernetes versions, one per entry or several
//!   comma-separated ones (`crate::state::cluster_versions_filter`), and a value
//!   that names none is refused rather than ignored: a listing that quietly dropped
//!   the filter would answer a page the caller did not ask for.
//! * `clusterType` — keeps only the rows for that cluster type.
//! * `defaultOnly` — keeps only the default version, the one `CreateCluster`
//!   creates when the caller names no `version`.
//! * `status` / `versionStatus` — the same lifecycle state in the deprecated and
//!   the current spelling (`status` "is deprecated. Use `versionStatus` instead,
//!   as that field matches for input and output of this action"), each matched
//!   against the row's derived status; giving both applies both. Only
//!   `versionStatus` matches in BOTH directions: this FILTER takes the deprecated
//!   member in the `ClusterVersionStatus` spelling (`standard-support`), while the
//!   reported `ClusterVersionInformation$status` carries the `VersionStatus`
//!   spelling its replacement does (see `crate::state`).
//! * `includeAll` — "Include all available versions in the response". It WIDENS the
//!   listing to the versions Amazon EKS no longer offers: an unfiltered call
//!   publishes only the versions still inside standard or extended support, because
//!   that is the set a caller picks a `CreateCluster` version from, so a row whose
//!   extended support has ended is reported only to a caller that asked for it —
//!   with `includeAll`, or with a `status`/`versionStatus` filter naming the
//!   unsupported state.
//! * `maxResults` / `nextToken` — pagination over the filtered rows.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeClusterVersions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Lists available Kubernetes versions for Amazon EKS clusters.
//!
//! ## Input
//!
//! * `clusterType` (String) — The type of cluster to filter versions by.
//! * `clusterVersions` (StringList) — List of specific cluster versions to describe.
//! * `defaultOnly` (BoxedBoolean) — Filter to show only default versions.
//! * `includeAll` (BoxedBoolean) — Include all available versions in the response.
//! * `maxResults` (DescribeClusterVersionMaxResults, range 1..100) — Maximum number of
//!   results to return.
//! * `nextToken` (String) — Pagination token for the next set of results.
//! * `status` (ClusterVersionStatus, one of: extended-support | standard-support |
//!   unsupported) — This field is deprecated. Use versionStatus instead, as that field
//!   matches for input and output of this action. Filter versions by their current status.
//! * `versionStatus` (VersionStatus, one of: EXTENDED_SUPPORT | STANDARD_SUPPORT |
//!   UNSUPPORTED) — Filter versions by their current status.
//!
//! ## Output
//!
//! * `clusterVersions` (ClusterVersionList) — List of cluster version information objects.
//! * `nextToken` (String) — Pagination token for the next set of results.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, ClusterVersionRecord, SupportStatus, CLUSTER_VERSIONS_LISTING, DEFAULT_LIST_PAGE_SIZE,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeClusterVersionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeClusterVersionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DescribeClusterVersions")]
#[derive(Debug)]
pub enum DescribeClusterVersionsErrorModeled {
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_cluster_versions::DescribeClusterVersionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeClusterVersionsError = crate::scaffolded::Scaffolded<DescribeClusterVersionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeClusterVersionsErrorModeled> for DescribeClusterVersionsError {
    fn from(modeled: DescribeClusterVersionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeClusterVersions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeClusterVersions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeClusterVersions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeClusterVersions<Eng>
{
    type Input = DescribeClusterVersionsInput;
    type Output = DescribeClusterVersionsOutput;
    type Error = DescribeClusterVersionsError;

    const NAME: &'static str = "DescribeClusterVersions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CLUSTER_VERSIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CLUSTER_VERSIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeClusterVersionsError::from(DescribeClusterVersionsErrorModeled::ServerException(
                ServerException {
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // The calendar is GLOBAL: a Kubernetes version is published for the service,
        // not per Region or per account, so nothing here reads a region or an
        // account from the request.
        let now = SystemTime::now();

        // Both spellings of the status filter name the SAME lifecycle state, so each
        // one given is resolved to that state and every resolved state must match.
        // A token neither modelled enum declares is refused by schema validation
        // before this handler runs, and refused here too rather than ignored.
        let mut status_filters: Vec<SupportStatus> = Vec::with_capacity(2);
        if let Some(token) = input.version_status.as_deref() {
            status_filters.push(
                SupportStatus::from_version_status(token)
                    .ok_or_else(unknown_version_status)?,
            );
        }
        if let Some(token) = input.status.as_deref() {
            status_filters.push(
                SupportStatus::from_cluster_version_status(token)
                    .ok_or_else(|| invalid_parameter(&state::cluster_version_status_message(token)))?,
            );
        }

        // A version whose extended support has ENDED is no longer one Amazon EKS
        // offers, so it belongs only to a listing that asked for it: `includeAll`
        // ("Include all available versions in the response") widens the listing to
        // every row the calendar holds, and a status filter naming the unsupported
        // state asks for exactly those rows.
        let include_unsupported = input.include_all.unwrap_or(false)
            || status_filters.contains(&SupportStatus::Unsupported);

        let requested_versions = match input.cluster_versions.as_deref() {
            None => Vec::new(),
            // "The list of your desired Kubernetes versions": the filter names
            // Kubernetes versions, which is the form its own refusal states —
            // `state::cluster_versions_filter` reads them and answers `None` for a
            // value that names none, which is refused rather than ignored. A filter
            // the service cannot read is not a filter it may silently drop: the page
            // it would answer is not the page the caller asked for.
            Some(requested) => state::cluster_versions_filter(requested)
                .ok_or_else(|| invalid_parameter(state::CLUSTER_VERSIONS_FILTER_MESSAGE))?,
        };
        let requested_type = input.cluster_type;
        let default_only = input.default_only.unwrap_or(false);

        // `maxResults` is modelled 1..100, so a value outside that range is REFUSED by
        // schema validation before this handler runs. Only the absent case gets a
        // value, and it is a whole page. The floor of 1 is NOT a second guess at that
        // validation: it is what makes the page arithmetic below total on its own,
        // rather than depending on a check another layer performs.
        let page_size = usize::try_from(input.max_results.unwrap_or(DEFAULT_LIST_PAGE_SIZE))
            .unwrap_or(DEFAULT_LIST_PAGE_SIZE as usize)
            .max(1);

        // A `nextToken` names the calendar row to resume AFTER. One this listing did
        // not issue — including a well-formed token another listing minted — is a bad
        // parameter, not an empty page, and it is refused in the sentence every
        // listing of this service states for one.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(CLUSTER_VERSIONS_LISTING, token)
                    .ok_or_else(|| invalid_parameter(state::INVALID_NEXT_TOKEN_MESSAGE))?,
            ),
        };

        let guard = store.lock();
        let matching: Vec<&ClusterVersionRecord> = guard
            .released_cluster_versions(now)
            .filter(|entry| {
                requested_versions.is_empty()
                    || requested_versions.contains(&entry.cluster_version)
            })
            .filter(|entry| {
                requested_type
                    .as_deref()
                    .is_none_or(|cluster_type| cluster_type == entry.cluster_type)
            })
            .filter(|entry| !default_only || entry.default_version)
            .filter(|entry| {
                let status = entry.support_status(now);
                status_filters.iter().all(|wanted| *wanted == status)
            })
            // Which keeps the listing consistent with each row's own dates: an
            // unfiltered call reports no row whose `endOfExtendedSupportDate` the
            // clock has passed, so no reported row contradicts the status derived
            // from it.
            .filter(|entry| {
                include_unsupported || entry.support_status(now) != SupportStatus::Unsupported
            })
            .collect();

        // The resume point is a position in THIS filtered listing: a token carried
        // over to a differently filtered call names a row that is not in it, which is
        // a bad parameter rather than a silent restart from the top.
        let start = match resume_after {
            None => 0,
            Some(key) => matching
                .iter()
                .position(|entry| entry.page_key() == key)
                .map(|position| position + 1)
                .ok_or_else(|| invalid_parameter(state::INVALID_NEXT_TOKEN_MESSAGE))?,
        };

        let remaining = &matching[start..];
        let returned = remaining.len().min(page_size);
        let cluster_versions: Vec<ClusterVersionInformation> = remaining[..returned]
            .iter()
            .map(|entry| entry.to_cluster_version_information(now))
            .collect();
        // A token is returned only when something is left to resume from, so the last
        // page omits it. Written with `checked_sub`/`get` so the arithmetic is total
        // whatever `returned` is, rather than relying on it being above zero.
        let next_token = (returned < remaining.len())
            .then(|| returned.checked_sub(1).and_then(|last| remaining.get(last)))
            .flatten()
            .map(|entry| {
                state::encode_scoped_page_token(CLUSTER_VERSIONS_LISTING, &entry.page_key())
            });

        Ok(DescribeClusterVersionsOutput {
            cluster_versions: Some(cluster_versions),
            next_token,
        })
    }
}

/// Refuse a parameter this operation cannot act on. `InvalidParameterException`
/// (HTTP 400) is the modelled "The specified parameter is invalid" channel.
fn invalid_parameter(message: &str) -> DescribeClusterVersionsError {
    DescribeClusterVersionsErrorModeled::InvalidParameterException(InvalidParameterException {
        message: Some(message.to_string()),
        ..Default::default()
    })
    .into()
}

/// Refuse a `versionStatus` whose token the modelled enum does not declare, naming
/// the member and the members that member accepts — the sentence this service states
/// for a member judged by an enum's value set
/// ([`state::member_must_be_one_of_message`]).
///
/// The DEPRECATED `status` spelling of the same filter states a refusal of its own
/// ([`state::cluster_version_status_message`]) rather than this one.
fn unknown_version_status() -> DescribeClusterVersionsError {
    let accepted: Vec<String> = SupportStatus::all()
        .iter()
        .map(|status| status.version_status().to_string())
        .collect();
    invalid_parameter(&state::member_must_be_one_of_message(
        "versionStatus",
        &accepted,
    ))
}
