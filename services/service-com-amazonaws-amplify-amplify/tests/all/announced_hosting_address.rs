// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS an app's hosting is announced at: `App$defaultDomain`, and whether the
//! emulator answers at the host a client composes from it.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `amplify` signing name, then a plain unsigned `GET` of the site
//! — so what is asserted is what a client that was told where its site is sees.
//!
//! The rule under test: an address this service ANNOUNCES is one THIS emulator answers
//! on, never a host on the zone the real service serves. `App$defaultDomain` is "the
//! default domain for the Amplify app" and `Branch$displayName` "is used as the default
//! domain prefix" (`smithy_query(query="doc",shape="App",member="defaultDomain")` /
//! `(shape="Branch",member="displayName")`), so a client FETCHES
//! `https://<displayName>.<defaultDomain>/` — which makes `defaultDomain` the address of
//! the emulator's own hosting: the endpoint the app was created through, under the name
//! RFC 6761 reserves for the loopback interface together with every name under it, so the
//! branch's own label resolves there too. An app created where this emulator has no such
//! address of its own — a request on one of the service's published hosts, on an emulator
//! that is served on no port at all — keeps the generated name under the hosting zone the
//! member documents.
//!
//! The table below spans the class that rule generalises over: every spelling of the
//! emulator's own endpoint a client is configured with (the IPv4 loopback address, the
//! loopback name, a bracketed IPv6 literal, each on its own port), and the service's own
//! published host. Each row asserts BOTH halves — the value announced, and that a `GET` of
//! the address built from it is answered by the site that branch deployed — because an
//! announced address nothing answers on is exactly the defect.
//!
//! The harness (`Emulator`) and the deployment driver (`Hosted`) are the ones
//! `app_branch_lifecycle` and `hosted_branch_site` already drive.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, REGION};
use crate::hosted_branch_site::{Hosted, SUCCEED};

/// The root document the rows deploy, so a `GET` of the announced address has something
/// to answer with.
const ROOT_DOCUMENT: &str = "index.html";

/// One row: the endpoint a client was pointed at, and the domain the app it creates there
/// announces.
struct Row {
    /// What the row is, named in every assertion.
    case: &'static str,
    /// The `Host` the control-plane calls address.
    endpoint: &'static str,
    /// The announced `defaultDomain`, or `None` when it is the generated name under the
    /// hosting zone (which carries the minted app id, so the row cannot spell it out).
    announced: Option<&'static str>,
}

/// The rows: every addressing a client of this emulator uses, and the service's own
/// published host beside them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the IPv4 loopback address with a port",
            endpoint: "127.0.0.1:44285",
            announced: Some("localhost:44285"),
        },
        Row {
            case: "the loopback name with another port",
            endpoint: "localhost:9297",
            announced: Some("localhost:9297"),
        },
        Row {
            case: "a bracketed IPv6 loopback",
            endpoint: "[::1]:38199",
            announced: Some("localhost:38199"),
        },
        Row {
            case: "the loopback address naming no port",
            endpoint: "127.0.0.1",
            announced: Some("localhost"),
        },
        Row {
            case: "this service's own published host",
            endpoint: "amplify.us-east-1.amazonaws.com",
            announced: None,
        },
    ]
}

/// The `app` record `GetApp` reports, addressed at `endpoint`.
async fn read_app(emulator: &Emulator, endpoint: &str, app_id: &str) -> Value {
    let reply = emulator
        .call_at(endpoint, "GET", &format!("/apps/{app_id}"), None)
        .await;
    assert_eq!(
        reply.status, 200,
        "app {app_id:?}: expected HTTP 200 from GetApp, got {reply:?}"
    );
    reply.member("app")
}

/// Rule: the domain an app announces is an address of THIS emulator, and the site of a
/// branch of that app is served at the host a client composes from it.
#[tokio::test]
async fn an_app_announces_a_hosting_address_this_emulator_answers_on() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let created = emulator
            .call_at(
                row.endpoint,
                "POST",
                "/apps",
                Some(&json!({ "name": format!("announced-{case}") })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateApp, got {created:?}"
        );
        let app = created.member("app");
        let app_id = string_member(&app, "appId");
        let announced = string_member(&app, "defaultDomain");

        let expected = row
            .announced
            .map_or_else(|| format!("{app_id}.amplifyapp.com"), str::to_string);
        assert_eq!(
            announced, expected,
            "case {case:?}: expected the announced domain {expected:?}, got {announced:?}"
        );
        // Whatever it names, it is never a host on the zone the real service serves —
        // unless this emulator has no address of its own to announce.
        assert_eq!(
            announced.ends_with(".amplifyapp.com"),
            row.announced.is_none(),
            "case {case:?}: the real service's hosting zone is only for an emulator with \
             no address of its own, got {announced:?}"
        );

        // The same value is reported by every reader, and by one addressed somewhere
        // else: it is the app's record, not a function of the reading request.
        let read_back = read_app(&emulator, row.endpoint, &app_id).await;
        assert_eq!(
            string_member(&read_back, "defaultDomain"),
            announced,
            "case {case:?}: GetApp must report the announced domain, got {read_back:?}"
        );
        let elsewhere = read_app(&emulator, &format!("amplify.{REGION}.amazonaws.com"), &app_id)
            .await;
        assert_eq!(
            string_member(&elsewhere, "defaultDomain"),
            announced,
            "case {case:?}: a read addressed elsewhere must report the same domain, got \
             {elsewhere:?}"
        );
        let listed = emulator.list_apps("").await;
        let first = list_member(&listed.body, "apps")
            .iter()
            .find(|listed| listed.get("appId").and_then(Value::as_str) == Some(app_id.as_str()))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: ListApps must report the created app"));
        assert_eq!(
            string_member(&first, "defaultDomain"),
            announced,
            "case {case:?}: ListApps must report the announced domain, got {first:?}"
        );

        // The announced address is one this emulator ANSWERS on: a branch of that app,
        // deployed to, serves its site at `<displayName>.<defaultDomain>` — the host
        // `Hosted` composes out of those two members exactly as a client does.
        let hosted = Hosted::adopt(emulator, app_id, &json!({ "branchName": "main" })).await;
        assert_eq!(
            hosted.host,
            format!("main.{announced}"),
            "case {case:?}: the site's host is the branch's label under the announced \
             domain, got {:?}",
            hosted.host
        );
        let body = format!("<html>{case}</html>").into_bytes();
        let status = hosted
            .deploy(case, &[(ROOT_DOCUMENT, body.as_slice())])
            .await;
        assert_eq!(
            status, SUCCEED,
            "case {case:?}: the deployment must succeed, got {status}"
        );
        let visited = hosted.visit("/").await;
        assert_eq!(
            visited.status, 200,
            "case {case:?}: a GET of the announced address must be answered, got {visited:?}"
        );
        assert_eq!(
            visited.body, body,
            "case {case:?}: expected the deployed document, got {:?}",
            String::from_utf8_lossy(&visited.body)
        );
    }
}
