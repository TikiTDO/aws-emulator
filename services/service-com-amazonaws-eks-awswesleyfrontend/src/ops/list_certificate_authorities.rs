// SPDX-License-Identifier: CC0-1.0

//! @op ListCertificateAuthorities
//! IMPLEMENTED. Walks the certificate authorities one cluster holds — the stored
//! [`crate::state::CertificateAuthorityRecord`]s on its
//! [`crate::state::ClusterRecord`], most recently created FIRST
//! ([`crate::state::ClusterRecord::certificate_authorities_newest_first`]) — and
//! reports each as the same `CertificateAuthoritySummary`
//! `CreateCertificateAuthority` answered with and `DescribeCertificateAuthority`
//! reads back, so a listing never disagrees with either about a CA's signing or
//! distribution status. The records are keyed by certificate-authority id, which is
//! the LOOKUP index the single-CA operations use, not this listing's order: a
//! caller rotating a cluster CA reads the successor it must distribute off the head
//! of the listing, so the head is the newest CA and the tail the outgoing signer.
//!
//! Paging follows `ListCapabilities`: a page holds at most `maxResults` summaries
//! ([`crate::state::DEFAULT_LIST_PAGE_SIZE`] when the caller gives none, the whole page
//! the model's 1..100 bound allows), and `nextToken` is returned only when something is
//! left — it names the newest-first order key
//! ([`crate::state::CertificateAuthorityRecord::page_key`]) of the certificate
//! authority to resume AFTER, so every page continues that same order and the token
//! stays valid across appends and deletes; a token this listing did not mint
//! (including one another listing issued, which is what the
//! [`crate::state::CERTIFICATE_AUTHORITIES_LISTING`] token scope rules out) is an
//! `InvalidParameterException` rather than an empty page.
//!
//! An unknown `clusterName` is the `ResourceNotFoundException` the model declares here,
//! never an empty page, which would claim a cluster that does not exist holds no
//! certificate authorities; a cluster `RegisterCluster` connected has no Amazon
//! EKS-managed control plane to hold one, so it is refused the way every other
//! certificate-authority operation refuses it.
//!
//! The one write this read makes is the `IN_PROGRESS` -> `COMPLETE` distribution move
//! the clock already owed each listed CA, applied the way
//! `DescribeCertificateAuthority` applies it: it was due at the stored instant whether
//! or not anybody called.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListCertificateAuthorities`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Lists the certificate authorities (CAs) for your cluster. A cluster has at most two
//! certificate authorities: the outgoing CA that's currently signing and, during a
//! rotation, one successor CA.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `maxResults` (CertificateAuthorityMaxResults, range 1..100) — The maximum number of
//!   results to return in a single call. To retrieve the remaining results, make another call
//!   with the returned nextToken value. If you don't specify a value, the default is 100
//!   results.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//!
//! ## Output
//!
//! * `certificateAuthorities` (CertificateAuthoritySummaryList) — A list of certificate
//!   authority summary objects, each containing basic information about a certificate
//!   authority, including its ID, signing status, and distribution status.
//! * `nextToken` (String) — The nextToken value to include in a future
//!   ListCertificateAuthorities request. When the results of a ListCertificateAuthorities
//!   request exceed maxResults, you can use this value to retrieve the next page of results.
//!   This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, CERTIFICATE_AUTHORITIES_LISTING, DEFAULT_LIST_PAGE_SIZE};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListCertificateAuthoritiesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListCertificateAuthoritiesError` alias below.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that
// shape and many of its operations declare it, but this one does not list it — and it
// is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]), which is
// the same rule `CreateCertificateAuthority` and `DescribeCertificateAuthority` keep.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#ListCertificateAuthorities",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum ListCertificateAuthoritiesErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_certificate_authorities::ListCertificateAuthoritiesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListCertificateAuthoritiesError = crate::scaffolded::Scaffolded<ListCertificateAuthoritiesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListCertificateAuthoritiesErrorModeled> for ListCertificateAuthoritiesError {
    fn from(modeled: ListCertificateAuthoritiesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListCertificateAuthorities` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListCertificateAuthorities<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListCertificateAuthorities<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListCertificateAuthorities<Eng>
{
    type Input = ListCertificateAuthoritiesInput;
    type Output = ListCertificateAuthoritiesOutput;
    type Error = ListCertificateAuthoritiesError;

    const NAME: &'static str = "ListCertificateAuthorities";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CERTIFICATE_AUTHORITIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CERTIFICATE_AUTHORITIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListCertificateAuthoritiesError::from(
                ListCertificateAuthoritiesErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let invalid_parameter = |message: String| {
            ListCertificateAuthoritiesError::from(
                ListCertificateAuthoritiesErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        // `maxResults` is modelled 1..100, so a value outside that range is REFUSED by
        // schema validation before this handler runs. Only the absent case gets a
        // value, and it is a whole page: "If you don't specify a value, the default is
        // 100 results." `.max(1)` is the floor that bound already guarantees, written
        // out so a page size of zero — which would answer an empty page with no
        // `nextToken`, a success claiming the cluster holds nothing — cannot be
        // reached even if the boundary validation changes.
        let page_size = input
            .max_results
            .unwrap_or(DEFAULT_LIST_PAGE_SIZE)
            .max(1) as usize;

        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // THE PARENT RESOURCE FIRST, so the one rule this resource has for an unknown
        // parent wins over anything else wrong with the request — a listing of a
        // cluster that does not exist is that 404 whether or not the token is also
        // malformed. A cluster name identifies a cluster only within one region, so
        // the cluster listed is the one in the region the request is signed for.
        let Some(cluster) = guard
            .clusters
            .get_mut(&(region, input.cluster_name.clone()))
        else {
            return Err(
                ListCertificateAuthoritiesErrorModeled::ResourceNotFoundException(
                    crate::state::cluster_not_found(&input.cluster_name),
                )
                .into(),
            );
        };

        // A cluster certificate authority is the Amazon EKS control plane's own CA, and
        // a cluster `RegisterCluster` connected has no such control plane: this
        // operation does not apply to it — a refusal rather than an empty listing,
        // because the listing itself is not something EKS keeps for that cluster.
        if cluster.is_connected() {
            return Err(ListCertificateAuthoritiesErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        // Materialise the distribution moves the clock already owes this cluster's
        // certificate authorities, so the summaries below report their state at `now`.
        // Each was due at its stored instant whether or not anybody called, so this
        // does not change what a later caller sees.
        for authority in cluster.certificate_authorities.values_mut() {
            authority.materialize_due_transitions(now);
        }

        // A `nextToken` names the certificate authority to resume AFTER. One this
        // listing did not issue is a bad parameter, not an empty page.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(CERTIFICATE_AUTHORITIES_LISTING, token)
                    .ok_or_else(|| {
                        invalid_parameter(state::INVALID_NEXT_TOKEN_MESSAGE.to_string())
                    })?,
            ),
        };

        // Materialise the cluster's certificate authorities in the order the listing
        // reports them — newest `createdAt` first, ties broken by id — BEFORE the page
        // is cut, so `maxResults` cuts that order rather than the id order the records
        // happen to be keyed under. The order key is a total, stable order over
        // exactly the CAs the cluster holds, which is what makes a token that names
        // the last key returned resume the same walk after an append or a delete.
        let ordered = cluster.certificate_authorities_newest_first();
        let mut remaining = ordered.into_iter().filter(|record| {
            resume_after
                .as_deref()
                .is_none_or(|after| record.page_key().as_str() > after)
        });
        let page: Vec<&crate::state::CertificateAuthorityRecord> =
            remaining.by_ref().take(page_size).collect();
        // A token is returned only when something is left to resume from, so the last
        // page omits it.
        let next_token = match (remaining.next(), page.last()) {
            (Some(_), Some(last)) => Some(state::encode_scoped_page_token(
                CERTIFICATE_AUTHORITIES_LISTING,
                &last.page_key(),
            )),
            _ => None,
        };
        let certificate_authorities: Vec<CertificateAuthoritySummary> =
            page.iter().map(|record| record.to_summary()).collect();

        Ok(ListCertificateAuthoritiesOutput {
            certificate_authorities: Some(certificate_authorities),
            next_token,
        })
    }
}
