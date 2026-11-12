// SPDX-License-Identifier: CC0-1.0

//! What this service reports in a modelled error list for a stored resource none of whose
//! modifications failed, driven over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`).
//!
//! The rule under test: every member of this model that reports the errors of a request
//! taken on a resource — `LastModificationErrors` on `Attachment`, `ConnectPeer` and
//! `Peering`, and `PolicyErrors` on `CoreNetworkPolicy` (`smithy_query query="shape"
//! shape="Attachment,ConnectPeer,Peering,CoreNetworkPolicy"`, each an optional list of
//! that resource's own error shape) — is reported PRESENT and EMPTY for a resource this
//! service accepted and stored, because the store holds every request taken on it and
//! none of them failed. An absent member would leave a client unable to tell "nothing
//! failed" from "nothing reported", and the answer may not depend on which resource kind,
//! which lifecycle state, or which operation is doing the reporting.
//!
//! The table spans that class: rows differing in resource kind (each of the four
//! attachment kinds, a transit gateway peering, a Connect peer, a core network policy), in
//! edge location, in whether the clock-owed settle span has elapsed before the read (so in
//! the lifecycle state the resource is read in) and in whether the resource carries tags.
//! Each row is then asserted through every operation that answers it: the create, the
//! dedicated reader, the listing where the kind has one, and the delete that removes it.
//! Nothing reaches into `crate::state`: every resource is created and read back through an
//! operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{core_network_at, policy_document};
use crate::core_network_policy_versions::{core_network_with_policy, get_policy, policy};
use crate::delete_attachment::{create_attachment_of_kind, Kind};
use crate::global_network_lifecycle::{Emulator, PAST_SETTLE};
use crate::resource_tagging::tag;
use crate::transit_gateway_peering::{
    create_peering, get_peering_ok, own_account, peering_id, transit_gateway_arn,
};

/// The member `Attachment`, `ConnectPeer` and `Peering` report their errors through.
const MODIFICATION_ERRORS: &str = "LastModificationErrors";

/// The member `CoreNetworkPolicy` reports its errors through: the same kind of member on
/// the one shape of this service that names it differently.
const POLICY_ERRORS: &str = "PolicyErrors";

/// Which resource a row builds.
#[derive(Clone, Copy, Debug)]
enum Subject {
    /// An attachment of one of the four kinds this service mints
    /// (`crate::delete_attachment::Kind`).
    Attachment(Kind),
    TransitGatewayPeering,
    ConnectPeer,
    CoreNetworkPolicy,
}

/// One row of the table.
#[derive(Clone, Copy, Debug)]
struct Case {
    /// Names the case in every assertion message.
    name: &'static str,
    subject: Subject,
    /// The Region of the core network's single edge location, and so of the resource.
    region: &'static str,
    /// Whether to let the settle span the clock owes elapse before the reads, which is what
    /// moves a started resource out of its transient state.
    settle: bool,
    /// Whether the resource carries tags.
    tags: bool,
}

/// The table: rows differing along everything the rule generalises over — the resource
/// kind, the edge location, which side of the clock-owed transition the read falls on, and
/// whether the resource carries tags.
const CASES: [Case; 10] = [
    Case {
        name: "a VPC attachment read while still CREATING, no tags",
        subject: Subject::Attachment(Kind::Vpc),
        region: "us-west-2",
        settle: false,
        tags: false,
    },
    Case {
        name: "a Connect attachment read once settled, with tags",
        subject: Subject::Attachment(Kind::Connect),
        region: "eu-central-1",
        settle: true,
        tags: true,
    },
    Case {
        name: "a site-to-site VPN attachment read while still CREATING, with tags",
        subject: Subject::Attachment(Kind::SiteToSiteVpn),
        region: "ap-southeast-4",
        settle: false,
        tags: true,
    },
    Case {
        name: "a Direct Connect gateway attachment read once settled, no tags",
        subject: Subject::Attachment(Kind::DirectConnectGateway),
        region: "us-east-1",
        settle: true,
        tags: false,
    },
    Case {
        name: "a transit gateway peering read while still CREATING, no tags",
        subject: Subject::TransitGatewayPeering,
        region: "us-west-2",
        settle: false,
        tags: false,
    },
    Case {
        name: "a transit gateway peering read once settled, with tags",
        subject: Subject::TransitGatewayPeering,
        region: "sa-east-1",
        settle: true,
        tags: true,
    },
    Case {
        name: "a Connect peer read while still CREATING, no tags",
        subject: Subject::ConnectPeer,
        region: "ap-south-1",
        settle: false,
        tags: false,
    },
    Case {
        name: "a Connect peer read once settled, with tags",
        subject: Subject::ConnectPeer,
        region: "us-west-2",
        settle: true,
        tags: true,
    },
    Case {
        name: "a core network policy read through every selector while its change set runs",
        subject: Subject::CoreNetworkPolicy,
        region: "eu-west-1",
        settle: false,
        tags: false,
    },
    Case {
        name: "a core network policy read through every selector once its change set settled",
        subject: Subject::CoreNetworkPolicy,
        region: "ca-central-1",
        settle: true,
        tags: false,
    },
];

/// The `Tags` the row carries, as a create body spells them (`Value::Null` for a create
/// naming no `Tags` member).
fn case_tags(case: &Case) -> Value {
    if case.tags {
        json!([tag("env", "test"), tag("owner", case.name)])
    } else {
        Value::Null
    }
}

/// Let the settle span elapse where the row asks for it.
async fn settle(case: &Case) {
    if case.settle {
        tokio::time::sleep(PAST_SETTLE).await;
    }
}

/// The one assertion of this suite: `resource` reports `member` as the empty list.
///
/// `path` names the operation that answered `resource`, so a failure says which reporting
/// path drifted from the rest.
fn assert_no_errors(case: &str, path: &str, member: &str, resource: &Value) {
    let actual = &resource[member];
    assert_eq!(
        *actual,
        json!([]),
        "case {case:?}: {path} must report {member} as the empty list of a stored resource \
         none of whose modifications failed, got {actual} in {resource}"
    );
}

/// An attachment of `kind`: its reader, the listing every attachment appears in, and the
/// delete that removes it.
async fn check_attachment(emulator: &Emulator, case: &Case, kind: Kind, index: usize) {
    let name = case.name;
    let (_, core_network) = core_network_at(emulator, name, &[case.region]).await;
    let (id, _) = create_attachment_of_kind(
        emulator,
        name,
        kind,
        &core_network,
        case.region,
        index,
        &case_tags(case),
    )
    .await;
    settle(case).await;

    let uri = kind.get_uri(&id);
    let (status, read) = emulator.get(&uri).await;
    assert_eq!(
        status, 200,
        "case {name:?}: GET {uri} expected 200, got {status} with {read}"
    );
    assert_no_errors(
        name,
        &format!("GET {uri}"),
        MODIFICATION_ERRORS,
        &read[kind.wrapper()]["Attachment"],
    );

    let (status, listed) = emulator.get("/attachments").await;
    assert_eq!(
        status, 200,
        "case {name:?}: ListAttachments expected 200, got {status} with {listed}"
    );
    let entry = listed["Attachments"]
        .as_array()
        .and_then(|attachments| {
            attachments
                .iter()
                .find(|attachment| attachment["AttachmentId"] == json!(id))
        })
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: no {id} among the attachments in {listed}"));
    assert_no_errors(name, "ListAttachments", MODIFICATION_ERRORS, &entry);

    let (status, deleted) = emulator.delete(&format!("/attachments/{id}")).await;
    assert_eq!(
        status, 200,
        "case {name:?}: DeleteAttachment expected 200, got {status} with {deleted}"
    );
    assert_no_errors(
        name,
        "DeleteAttachment",
        MODIFICATION_ERRORS,
        &deleted["Attachment"],
    );
}

/// A transit gateway peering: its create, its reader and the delete that removes it.
async fn check_peering(emulator: &Emulator, case: &Case, index: usize) {
    let name = case.name;
    let (_, core_network) = core_network_at(emulator, name, &[case.region]).await;
    let mut body = json!({
        "CoreNetworkId": core_network,
        "TransitGatewayArn": transit_gateway_arn(
            "aws",
            case.region,
            &own_account(),
            index + 1,
            8,
        ),
    });
    if case.tags {
        body["Tags"] = case_tags(case);
    }
    let (status, created) = create_peering(emulator, body).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateTransitGatewayPeering expected 200, got {status} with {created}"
    );
    let wrapper = created["TransitGatewayPeering"].clone();
    assert_no_errors(
        name,
        "CreateTransitGatewayPeering",
        MODIFICATION_ERRORS,
        &wrapper["Peering"],
    );
    let id = peering_id(name, &wrapper);
    settle(case).await;

    let read = get_peering_ok(emulator, name, &id).await;
    assert_no_errors(
        name,
        "GetTransitGatewayPeering",
        MODIFICATION_ERRORS,
        &read["Peering"],
    );

    let (status, deleted) = emulator.delete(&format!("/peerings/{id}")).await;
    assert_eq!(
        status, 200,
        "case {name:?}: DeletePeering expected 200, got {status} with {deleted}"
    );
    assert_no_errors(name, "DeletePeering", MODIFICATION_ERRORS, &deleted["Peering"]);
}

/// A Connect peer of a Connect attachment: its create, its reader and the delete that
/// removes it.
async fn check_connect_peer(emulator: &Emulator, case: &Case, index: usize) {
    let name = case.name;
    let (_, core_network) = core_network_at(emulator, name, &[case.region]).await;
    let (attachment, _) = create_attachment_of_kind(
        emulator,
        name,
        Kind::Connect,
        &core_network,
        case.region,
        index,
        &Value::Null,
    )
    .await;

    let mut body = json!({
        "ConnectAttachmentId": attachment,
        "PeerAddress": "203.0.113.7",
        "CoreNetworkAddress": "198.51.100.7",
        "InsideCidrBlocks": ["169.254.17.0/29"],
        "BgpOptions": { "PeerAsn": 65_001 },
    });
    if case.tags {
        body["Tags"] = case_tags(case);
    }
    let (status, created) = emulator.post("/connect-peers", body).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateConnectPeer expected 200, got {status} with {created}"
    );
    assert_no_errors(
        name,
        "CreateConnectPeer",
        MODIFICATION_ERRORS,
        &created["ConnectPeer"],
    );
    let id = created["ConnectPeer"]["ConnectPeerId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no ConnectPeer.ConnectPeerId in {created}"))
        .to_string();
    settle(case).await;

    let (status, read) = emulator.get(&format!("/connect-peers/{id}")).await;
    assert_eq!(
        status, 200,
        "case {name:?}: GetConnectPeer expected 200, got {status} with {read}"
    );
    assert_no_errors(
        name,
        "GetConnectPeer",
        MODIFICATION_ERRORS,
        &read["ConnectPeer"],
    );

    let (status, deleted) = emulator.delete(&format!("/connect-peers/{id}")).await;
    assert_eq!(
        status, 200,
        "case {name:?}: DeleteConnectPeer expected 200, got {status} with {deleted}"
    );
    assert_no_errors(
        name,
        "DeleteConnectPeer",
        MODIFICATION_ERRORS,
        &deleted["ConnectPeer"],
    );
}

/// A core network policy, read through every selector `GetCoreNetworkPolicy` accepts: the
/// shape names its error list `PolicyErrors`, and the same rule reaches it.
async fn check_policy(emulator: &Emulator, case: &Case) {
    let name = case.name;
    let core_network =
        core_network_with_policy(emulator, name, &policy_document(&[case.region])).await;
    settle(case).await;

    for query in ["", "?alias=LIVE", "?alias=LATEST", "?policyVersionId=1"] {
        let (status, read) = get_policy(emulator, &core_network, query).await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetCoreNetworkPolicy({query:?}) expected 200, got {status} with {read}"
        );
        assert_no_errors(
            name,
            &format!("GetCoreNetworkPolicy({query:?})"),
            POLICY_ERRORS,
            policy(&read),
        );
    }
}

#[tokio::test]
async fn every_modelled_error_list_is_empty_for_a_resource_this_service_stored() {
    for (index, case) in CASES.iter().enumerate() {
        let emulator = Emulator::new();
        match case.subject {
            Subject::Attachment(kind) => check_attachment(&emulator, case, kind, index).await,
            Subject::TransitGatewayPeering => check_peering(&emulator, case, index).await,
            Subject::ConnectPeer => check_connect_peer(&emulator, case, index).await,
            Subject::CoreNetworkPolicy => check_policy(&emulator, case).await,
        }
    }
}
