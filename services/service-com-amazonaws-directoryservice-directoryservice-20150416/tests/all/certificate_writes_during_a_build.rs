// SPDX-License-Identifier: CC0-1.0

//! What a directory's certificate inventory does while the directory is still being built.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so nothing here reaches into the state struct and
//! an answer that is not computed from persisted state cannot pass. The harness, the stage
//! names, the wait that lands inside a build and the sentence a directory a call cannot
//! reach is refused with are the sibling suites' (`crate::directory_lifecycle`,
//! `crate::directory_build_stages`, `crate::certificate_functions`) rather than second
//! copies.
//!
//! The rule under test: a call that would CHANGE which certificates a directory holds needs
//! that directory in service, so from every stage of the chain a create owes — the stage the
//! request is accepted in and the stage the directory's servers are built in — it is refused
//! with the modelled `DirectoryUnavailableException`, stated in this service's sentence for
//! a directory a call cannot reach, and leaves the inventory exactly as it was; the READ that
//! reports that inventory is served from every stage, because reporting what a directory
//! holds needs nothing of the directory's servers.
//!
//! It is asserted over a grid: every `DirectoryType` this service creates (two of which do
//! not offer the function the certificate performs at all, so the rows also pin that the
//! stage is read BEFORE what the type offers), both transient stages of the chain, and every
//! way a caller asks for a change to the inventory — a registration naming each value of the
//! modelled `CertificateType`, a registration naming none (taking the documented default),
//! and a deregistration. The sibling suite `directory_build_stages` asserts the same gate
//! across the OPERATION families of one directory; this one asserts it across the directory
//! types and the whole certificate surface, and asserts what the refusals left behind.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ad_connector::minimal_connect;
use crate::certificate_functions::listed;
use crate::certificates::{certificate_pem, TYPE_CLIENT_CERT_AUTH, TYPE_CLIENT_LDAPS};
use crate::directory_build_stages::{directory_unavailable, refused_with, INSIDE_THE_BUILD};
use crate::directory_lifecycle::{
    call, describe_one, minimal_create, ok, service, string, PAST_ONE_SPAN, STAGE_ACTIVE,
    STAGE_CREATING, STAGE_REQUESTED,
};
use crate::microsoft_ad::minimal_microsoft_ad;

/// A `CertificateId` the modelled pattern `^c-[0-9a-f]{10}$` admits
/// (`smithy_query(query="operation",operation="DeregisterCertificate")`) that no
/// registration of these directories answers to, so a deregistration that got past the
/// stage would be refused on the certificate rather than on the shape of the id it named.
const UNREGISTERED_CERTIFICATE: &str = "c-00112233aa";

/// Assert that `directory_id` is in `expected_stage`, that the inventory is READ from it,
/// that every way of changing that inventory is refused on the stage, and that the refusals
/// left the inventory as they found it.
async fn the_inventory_survives_every_write(
    router: &MultiServiceRouter,
    directory_id: &str,
    expected_stage: &str,
    pem: &str,
    kind: &str,
) {
    let case = format!("{kind} in {expected_stage}");
    let description = describe_one(router, directory_id, &case).await;
    assert_eq!(
        description["Stage"],
        json!(expected_stage),
        "case {case:?}: expected the directory in {expected_stage}, got {description}"
    );

    // The inventory is READ from this stage: a directory still being built holds no
    // registrations, which is an empty list rather than a refusal.
    let before = listed(router, directory_id, &case).await;
    assert!(
        before.is_empty(),
        "case {case:?}: expected a directory still being built to report no registrations, \
         got {before:?}"
    );

    // (the operation, what the row asks for, its body) — every way a caller asks for a
    // change to the inventory.
    let writes: [(&str, &str, Value); 4] = [
        (
            "RegisterCertificate",
            "no type named",
            json!({ "DirectoryId": directory_id, "CertificateData": pem }),
        ),
        (
            "RegisterCertificate",
            "LDAPS named",
            json!({
                "DirectoryId": directory_id,
                "CertificateData": pem,
                "Type": TYPE_CLIENT_LDAPS
            }),
        ),
        (
            "RegisterCertificate",
            "client authentication named",
            json!({
                "DirectoryId": directory_id,
                "CertificateData": pem,
                "Type": TYPE_CLIENT_CERT_AUTH
            }),
        ),
        (
            "DeregisterCertificate",
            "a deregistration",
            json!({ "DirectoryId": directory_id, "CertificateId": UNREGISTERED_CERTIFICATE }),
        ),
    ];
    for (operation, asked_for, body) in writes {
        refused_with(
            router,
            operation,
            body,
            "DirectoryUnavailableException",
            &directory_unavailable(directory_id),
            &format!("{case}: {operation}, {asked_for}"),
        )
        .await;
    }

    // The refusals left the directory as they found it: a gate that refused and wrote would
    // have put a registration in here.
    let after = listed(router, directory_id, &case).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the refused writes to leave the inventory alone, got {after:?}"
    );
}

/// RULE: a write to a directory's certificate inventory is served only while the directory
/// is in service — from any other stage of the chain its create owes it is refused with the
/// modelled `DirectoryUnavailableException` and stores nothing — while the read that reports
/// the inventory is served from every stage.
#[tokio::test]
async fn a_certificate_write_during_a_build_is_refused_and_leaves_the_inventory_alone() {
    let router = service();

    // The certificate is built BEFORE the creates below, so generating its key pair cannot
    // eat into the window the first transition a create owes falls due in.
    let (pem, _) = certificate_pem(Some("build.example.com"), (2034, 1, 2));

    // The rows span every directory type this service creates: Simple AD and AD Connector
    // alongside Managed Microsoft AD, so what refuses the calls below is the stage the
    // directory is in rather than anything about one create.
    let rows: [(&str, &str, Value); 3] = [
        (
            "a Simple AD directory",
            "CreateDirectory",
            minimal_create("simple.build.example.com"),
        ),
        (
            "an AD Connector",
            "ConnectDirectory",
            minimal_connect("connector.build.example.com"),
        ),
        (
            "a Managed Microsoft AD directory",
            "CreateMicrosoftAD",
            minimal_microsoft_ad("managed.build.example.com"),
        ),
    ];

    // The stage a create's request is accepted in is the SHORTEST of the chain, so each row
    // is asserted in it straight after its own create rather than after all three.
    let mut created: Vec<(&str, String)> = Vec::new();
    for (kind, operation, body) in rows {
        let answer = ok(&router, operation, body, kind).await;
        let id = string(&answer, "DirectoryId", kind);
        the_inventory_survives_every_write(&router, &id, STAGE_REQUESTED, &pem, kind).await;
        created.push((kind, id));
    }

    // The stage the directory's servers are built in — the one a caller polling a create
    // spends the build in — is reached without anybody writing, and refuses the same calls.
    tokio::time::sleep(INSIDE_THE_BUILD).await;
    for (kind, id) in &created {
        the_inventory_survives_every_write(&router, id, STAGE_CREATING, &pem, kind).await;
    }

    // Once the chain has run out the stage refuses nothing: what the same call answers is
    // then decided on the certificate and on the function the directory's type offers, and
    // the inventory reports exactly what that answer stored.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (kind, id) in &created {
        let case = format!("{kind} in service");
        let description = describe_one(&router, id, &case).await;
        assert_eq!(
            description["Stage"],
            json!(STAGE_ACTIVE),
            "case {case:?}: expected the stage the chain ends on, got {description}"
        );

        let (status, parsed) = call(
            &router,
            "RegisterCertificate",
            json!({ "DirectoryId": id, "CertificateData": &pem, "Type": TYPE_CLIENT_LDAPS }),
        )
        .await;
        let code = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            !code.contains("DirectoryUnavailableException"),
            "case {case:?}: expected the stage to refuse nothing once the directory is in \
             service, got {status} with {parsed}"
        );

        let stored: Vec<String> = if status == 200 {
            vec![string(&parsed, "CertificateId", &case)]
        } else {
            Vec::new()
        };
        let after = listed(&router, id, &case).await;
        assert_eq!(
            after, stored,
            "case {case:?}: expected the inventory to report what the registration stored \
             ({stored:?}), got {after:?}"
        );
    }
}
