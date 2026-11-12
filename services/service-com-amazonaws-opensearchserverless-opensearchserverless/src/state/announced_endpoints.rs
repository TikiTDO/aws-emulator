// SPDX-License-Identifier: CC0-1.0

//! The endpoints a collection publishes until the address this emulator SERVES it at is
//! opened, and for an emulator that can open no address of its own.
//!
//! `CollectionDetail$collectionEndpoint` is the "Collection-specific endpoint used to
//! submit index, search, and data upload requests to an OpenSearch Serverless collection",
//! and `dashboardEndpoint` / `fipsEndpoints` are the same address in its other forms: none
//! of them is a label a caller reads back, each is an address a client then DIALS. What a
//! collection PUBLISHES is therefore its own listener ([`super::collection_plane`]), which
//! a stock OpenSearch client can be configured with directly. This module answers the
//! question that is left: what a collection reports when there is no such listener —
//! between the create that stored it and the address being opened, and on an emulator that
//! can open no listener at all (a host driving it on a runtime with no IO).
//!
//! Two forms can stand there, and [`AnnouncedEndpoints::of_collection`] picks between them
//! with the tree's one reading of the question
//! (`service_common::data_plane_addressing::reached_emulator_endpoint` /
//! `served_endpoint_authority`):
//!
//! * the EMULATOR's own endpoint — the one this request reached, else the port this process
//!   is served on — with the collection behind the reserved namespace segment that names
//!   this plane (`/_aws/aoss/{collection-id}`), which is the prefixed path-style addressing
//!   [`crate::service::collection_rest`] serves at the front door. A client on this machine
//!   resolves no host but the one it is already talking to, so this is an address it can
//!   reach;
//! * AWS's own endpoint hosts ([`super::collection_endpoint`] and the three beside it) for
//!   a request that addressed one of AWS's own hosts on an emulator served on no port at
//!   all: there is no own address to announce in their place, and the data plane claims
//!   those hosts host-style.
//!
//! The FIPS pair carries the host label the model's own FIPS endpoint template uses
//! ([`super::FIPS_ARN_NAMESPACE`]) wherever the standard pair carries
//! [`super::ARN_NAMESPACE`], in the host for the AWS forms and in the plane segment for the
//! emulator's own, so the four published addresses stay four distinct addresses and every
//! one of them resolves to this collection.

use ::service_common::data_plane_addressing::{
    reached_emulator_endpoint, served_endpoint_authority, EMULATOR_PATH_NAMESPACE,
};

use super::{
    collection_endpoint, dashboard_endpoint, fips_collection_endpoint, fips_dashboard_endpoint,
    ARN_NAMESPACE, DASHBOARDS_PATH, FIPS_ARN_NAMESPACE,
};

/// The scheme an announced endpoint carries. A collection endpoint is dialled over TLS,
/// and this emulator serves its whole surface over TLS as well.
const ANNOUNCED_SCHEME: &str = "https";

/// The four endpoints one collection publishes, in the form this emulator announces them.
#[derive(Debug, Clone)]
pub struct AnnouncedEndpoints {
    /// `CollectionDetail$collectionEndpoint`.
    pub collection: String,
    /// `CollectionDetail$dashboardEndpoint`.
    pub dashboards: String,
    /// `FipsEndpoints$collectionEndpoint`.
    pub fips_collection: String,
    /// `FipsEndpoints$dashboardEndpoint`.
    pub fips_dashboards: String,
}

impl AnnouncedEndpoints {
    /// The four addresses a collection minted by THIS request reports until its own
    /// listener is opened (see the module docs for which form stands for whom).
    pub fn of_collection(
        collection_id: &str,
        region: &str,
        parts: &::http::request::Parts,
        served_base: &str,
    ) -> Self {
        match emulator_authority(parts, served_base) {
            Some(authority) => Self {
                collection: announced_base(&authority, ARN_NAMESPACE, collection_id),
                dashboards: dashboards_of(&announced_base(
                    &authority,
                    ARN_NAMESPACE,
                    collection_id,
                )),
                fips_collection: announced_base(&authority, FIPS_ARN_NAMESPACE, collection_id),
                fips_dashboards: dashboards_of(&announced_base(
                    &authority,
                    FIPS_ARN_NAMESPACE,
                    collection_id,
                )),
            },
            None => Self {
                collection: collection_endpoint(collection_id, region),
                dashboards: dashboard_endpoint(collection_id, region),
                fips_collection: fips_collection_endpoint(collection_id, region),
                fips_dashboards: fips_dashboard_endpoint(collection_id, region),
            },
        }
    }
}

/// The address this emulator answers on for the client that made this request: the
/// emulator's own endpoint the request reached, else the port this process is served on,
/// and `None` for a request that addressed one of AWS's own hosts on an emulator served on
/// no port at all — which has no own address to announce.
fn emulator_authority(parts: &::http::request::Parts, served_base: &str) -> Option<String> {
    reached_emulator_endpoint(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
    .or_else(|| served_endpoint_authority(served_base))
}

/// `https://<authority>/_aws/<plane>/<collection id>` — the collection's own base on an
/// address this emulator answers on, in the prefixed path-style addressing
/// [`crate::service::collection_rest`] resolves.
fn announced_base(authority: &str, plane: &str, collection_id: &str) -> String {
    format!(
        "{ANNOUNCED_SCHEME}://{authority}/{EMULATOR_PATH_NAMESPACE}/{plane}/{collection_id}"
    )
}

/// The Dashboards endpoint of a collection base, which is that base plus the path AWS's
/// own Dashboards endpoint adds to a collection endpoint ([`DASHBOARDS_PATH`]).
fn dashboards_of(base: &str) -> String {
    format!("{base}{DASHBOARDS_PATH}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request parts of a call that addressed `host`.
    fn addressed(host: &str) -> ::http::request::Parts {
        ::http::Request::builder()
            .uri("/")
            .header(::http::header::HOST, host)
            .body(())
            .expect("a well-formed request")
            .into_parts()
            .0
    }

    /// RULE: until a collection's own listener is open, it reports the emulator's own
    /// endpoint — the one the request reached, else the port this process is served on —
    /// with the collection named in the path, and AWS's own hosts only when there is no own
    /// address to report in their place.
    #[test]
    fn a_collection_reports_the_emulators_own_address_wherever_there_is_one() {
        /// One case: who called, where this emulator is served, and the collection endpoint
        /// that stands.
        struct Row {
            case: &'static str,
            host: String,
            served_base: &'static str,
            collection: String,
        }
        let id = "abc123";
        let served = "https://127.0.0.1:39355";
        let aws_host = format!("{ARN_NAMESPACE}.us-west-2.amazonaws.com");
        let rows: Vec<Row> = vec![
            Row {
                case: "a client on the emulator's own loopback endpoint",
                host: "127.0.0.1:9297".to_string(),
                served_base: served,
                collection: format!("https://127.0.0.1:9297/{EMULATOR_PATH_NAMESPACE}/{ARN_NAMESPACE}/{id}"),
            },
            Row {
                case: "a client on a loopback name, this emulator served on no port",
                host: "localhost:9297".to_string(),
                served_base: ::smithy_server_runtime::UNSERVED_URL_BASE,
                collection: format!("https://localhost:9297/{EMULATOR_PATH_NAMESPACE}/{ARN_NAMESPACE}/{id}"),
            },
            Row {
                case: "a client that resolves AWS's own hosts, this emulator served",
                host: aws_host.clone(),
                served_base: served,
                collection: format!("https://127.0.0.1:39355/{EMULATOR_PATH_NAMESPACE}/{ARN_NAMESPACE}/{id}"),
            },
            Row {
                case: "a client that resolves AWS's own hosts, served on no port at all",
                host: aws_host.clone(),
                served_base: ::smithy_server_runtime::UNSERVED_URL_BASE,
                collection: collection_endpoint(id, "us-west-2"),
            },
        ];
        for Row {
            case,
            host,
            served_base,
            collection,
        } in &rows
        {
            let announced =
                AnnouncedEndpoints::of_collection(id, "us-west-2", &addressed(host), served_base);
            assert_eq!(
                &announced.collection, collection,
                "case {case:?}: expected the collection endpoint {collection:?}, got {:?}",
                announced.collection
            );
            // The Dashboards endpoint is the collection endpoint plus the documented path,
            // and the FIPS pair is the same two addresses under the FIPS label, so the four
            // published addresses are four distinct addresses.
            assert_eq!(
                announced.dashboards,
                dashboards_of(collection),
                "case {case:?}: the Dashboards endpoint is the collection endpoint plus \
                 {DASHBOARDS_PATH}, got {:?}",
                announced.dashboards
            );
            assert_eq!(
                announced.fips_dashboards,
                dashboards_of(&announced.fips_collection),
                "case {case:?}: the FIPS Dashboards endpoint is the FIPS endpoint plus \
                 {DASHBOARDS_PATH}, got {:?}",
                announced.fips_dashboards
            );
            assert!(
                announced.fips_collection.contains(FIPS_ARN_NAMESPACE)
                    && announced.fips_collection != announced.collection,
                "case {case:?}: the FIPS endpoint carries the {FIPS_ARN_NAMESPACE} label of \
                 its own, got {:?}",
                announced.fips_collection
            );
            // Which addressing resolves each form: a form naming the collection in its
            // PATH shares its authority with the emulator's own front door, so it is
            // resolved by that path and never by its host
            // (`super::CollectionRecord::publishes_host`), while AWS's own host is the
            // collection's whole address.
            let whole_host = crate::state::addresses_a_whole_host(&announced.collection);
            assert_eq!(
                whole_host,
                !announced.collection.contains(EMULATOR_PATH_NAMESPACE),
                "case {case:?}: expected whole-host addressing only for the form that names \
                 no path, got {whole_host} for {:?}",
                announced.collection
            );
        }
    }
}
