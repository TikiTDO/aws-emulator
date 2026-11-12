// SPDX-License-Identifier: CC0-1.0

//! The HOST a `redirect` action's `#{host}` resolves to: the host the CLIENT addressed the
//! load balancer by, whichever way the request reached this plane.
//!
//! The rule under test: **every answer that reports the host of the request — the `#{host}` a
//! redirect action substitutes into its `Location` — carries the name the CLIENT dialled, and
//! never an endpoint of this API's. `RedirectActionConfig$Host` is "[t]he hostname. This
//! component is not percent-encoded. The hostname can contain `#{host}`", one of the reserved
//! keywords "you can reuse URI components" with, so the value substituted is read off the
//! request being redirected. A request a load balancer's own socket accepted is handed to
//! this plane re-addressed to this service's own endpoint, so for such a request the host the
//! client dialled is the one the load balancer PUBLISHES (`LoadBalancer$DNSName`, "the public
//! DNS name of the load balancer") — not the endpoint it was re-addressed to.**
//!
//! The rows differ along the ADDRESSING the request reached the plane by: the published name
//! itself, the reserved data-plane path under this API's own endpoint (the addressing the
//! load balancer's socket forwards a client's request as, with and without the listener's
//! port on it, and carrying the host the client dialled), and the reserved path or first
//! segment under the emulator's own endpoint, which is a client's own addressing and stays
//! exactly as it arrived.

use std::time::{Duration, SystemTime};

use crate::alb_data_plane::{create_load_balancer, send, settle};
use crate::provisioning_loop::{action, call, emulator, p};

/// The host a client addresses the emulator's own endpoint by, which is the only address a
/// client on the machine this emulator runs on can reach.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The endpoint of THIS API, which is the host a request forwarded from a load balancer's own
/// socket to this plane carries — and the one value `#{host}` may never resolve to.
const SERVICE_ENDPOINT: &str = "elasticloadbalancingv2.amazonaws.com";

/// The load balancer's name, and the listener port its one HTTP listener serves.
const NAME: &str = "redirect-host-alb";
const LISTENER_PORT: &str = "80";

/// The path a row requests, and the `Location` components the action is configured with.
const REQUESTED: &str = "/deep/thing";
const REDIRECT_PATH: &str = "/moved#{path}";
const REDIRECT_QUERY: &str = "orig=#{path}";

#[tokio::test]
async fn a_redirect_substitutes_the_host_the_client_addressed_and_never_this_apis_endpoint() {
    let (router, engine) = emulator();
    let (lb_arn, dns_name) = create_load_balancer(&router, NAME).await;

    // The HTTP listener whose DEFAULT action redirects, keeping the request's own host and
    // path: "#{protocol}, #{host}, #{port}, #{path} ... and #{query}" are the reserved
    // keywords, and this action reuses two of them.
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", LISTENER_PORT));
    form.push(p("DefaultActions.member.1.Type", "redirect"));
    form.push(p(
        "DefaultActions.member.1.RedirectConfig.Protocol",
        "HTTPS",
    ));
    form.push(p("DefaultActions.member.1.RedirectConfig.Host", "#{host}"));
    form.push(p(
        "DefaultActions.member.1.RedirectConfig.Path",
        REDIRECT_PATH,
    ));
    form.push(p(
        "DefaultActions.member.1.RedirectConfig.Query",
        REDIRECT_QUERY,
    ));
    form.push(p(
        "DefaultActions.member.1.RedirectConfig.StatusCode",
        "HTTP_301",
    ));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup CreateListener: expected 200, got {} body {}",
        reply.status, reply.body
    );

    // The move to `active` the clock owes the load balancer, placed rather than slept for.
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));

    // The published name without the port the socket answers on, which is the HOST a client
    // dialling that name addressed.
    let published_host = dns_name.rsplit_once(':').map_or(dns_name.as_str(), |(host, _)| host);
    let prefixed = format!("/_aws/elasticloadbalancing/{dns_name}{REQUESTED}");
    let by_name = format!("/_aws/elasticloadbalancing/{NAME}{REQUESTED}");
    let path_style = format!("/{NAME}{REQUESTED}");
    let forwarded_host = format!("{SERVICE_ENDPOINT}:{LISTENER_PORT}");

    // Each row is one addressing, the host it carries, the headers it carries with it, and
    // the host `#{host}` must resolve to: the client's own addressing where the request
    // carries one, and the published name where the host it carries is an endpoint of this
    // API's and nothing carried the client's own.
    struct Row<'a> {
        case: &'a str,
        /// The `Host` the request carries.
        host: &'a str,
        /// The path, within whatever that host is.
        target: &'a str,
        /// The other headers it carries.
        headers: &'a [(&'a str, &'a str)],
        /// The host `#{host}` must resolve to.
        expected_host: &'a str,
    }
    let rows: Vec<Row<'_>> = vec![
        Row {
            case: "the published name, dialled by a client",
            host: dns_name.as_str(),
            target: REQUESTED,
            headers: &[],
            expected_host: published_host,
        },
        Row {
            case: "the reserved path under this API's endpoint, carrying the listener's port",
            host: forwarded_host.as_str(),
            target: prefixed.as_str(),
            headers: &[],
            expected_host: published_host,
        },
        Row {
            case: "the reserved path under this API's endpoint, carrying no port",
            host: SERVICE_ENDPOINT,
            target: by_name.as_str(),
            headers: &[],
            expected_host: published_host,
        },
        Row {
            // What the load balancer's own socket forwards: re-addressed to this API, with
            // the name the client dialled carried beside it — a custom name a client
            // resolves to the load balancer, which is what the keyword must report.
            case: "the socket's own leg, carrying the host the client dialled",
            host: forwarded_host.as_str(),
            target: prefixed.as_str(),
            headers: &[("x-forwarded-host", "shop.example.com")],
            expected_host: "shop.example.com",
        },
        Row {
            case: "the reserved path under the emulator's own endpoint",
            host: EMULATOR_HOST,
            target: prefixed.as_str(),
            headers: &[],
            expected_host: "127.0.0.1",
        },
        Row {
            case: "the name as the first path segment of the emulator's own endpoint",
            host: EMULATOR_HOST,
            target: path_style.as_str(),
            headers: &[],
            expected_host: "127.0.0.1",
        },
    ];
    for row in &rows {
        let Row {
            case,
            host,
            target,
            headers,
            expected_host,
        } = row;
        let answered = send(&router, "GET", host, target, headers).await;
        assert_eq!(
            answered.status, 301,
            "case {case:?}: expected HTTP 301, got {} with {:?}",
            answered.status,
            answered.header("location")
        );
        // The port is the LISTENER's, the path and query the request's own, and the host the
        // one the client addressed.
        let expected = format!(
            "https://{expected_host}:{LISTENER_PORT}/moved{path}?orig={path}",
            path = REQUESTED.trim_start_matches('/')
        );
        assert_eq!(
            answered.header("location"),
            Some(expected.as_str()),
            "case {case:?}: expected Location {expected:?}, got {:?}",
            answered.header("location")
        );
        assert!(
            !answered
                .header("location")
                .unwrap_or_default()
                .contains(SERVICE_ENDPOINT),
            "case {case:?}: the redirect must not send a client to this API's endpoint, got {:?}",
            answered.header("location")
        );
    }
}
