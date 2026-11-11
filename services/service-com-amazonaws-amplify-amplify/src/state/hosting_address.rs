// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS an app's hosting is announced at, and the host one branch's site answers
//! on.
//!
//! `App$defaultDomain` is "the default domain for the Amplify app"
//! (`smithy_query(query="doc",shape="App",member="defaultDomain")`) and
//! `Branch$displayName` "is used as the default domain prefix"
//! (`smithy_query(query="doc",shape="Branch",member="displayName")`), so a client that
//! was told both FETCHES `https://<displayName>.<defaultDomain>/` — the deployed page,
//! the branch's basic-authorization challenge, or the redirect a rule states. That makes
//! `defaultDomain` an address, and an address a service announces has to be one the
//! emulator that announced it answers on: a name under the hosting zone AWS serves
//! resolves to AWS, or to nothing, on the machine this emulator runs on, so every
//! hosting-dependent flow would leave the emulator entirely.
//!
//! So the domain an app announces is the PORT this emulator is reached on
//! ([`announced_default_domain`]), under which a branch's site answers with the branch as
//! the host's first label — the addressing `crate::service`'s hosting edge claims and
//! serves. What is announced is that port under the LOOPBACK NAME rather than under the
//! address itself, because the client prefixes the branch as one more DNS LABEL: no
//! resolver answers a name under an IP address, while RFC 6761 reserves `localhost` and
//! every name under it to the loopback interface, which is where this emulator's own
//! endpoint answers. An emulator reached anywhere else — served on no port at all (one
//! embedded in another process, or a test), or on an interface whose address names no such
//! tree — has no address a branch label can be prefixed to, and reports the generated name
//! under Amplify's own zone instead. The reading of which host is this emulator's own, of
//! the authority it is served at, and of whether that authority is the loopback one, is the
//! workspace's and the core's (`::service_common::data_plane_addressing`,
//! `::emulator_server::is_loopback_host`).
//!
//! The same two members decide which requests the edge serves, so the host a branch
//! answers on is composed HERE too ([`hosted_branch_host`]), from the stored record, and
//! compared against the host a request addressed in one normal form
//! ([`hosted_site_host`]): the announced domain carries the endpoint's port, and a host is
//! the same host however its port, its userinfo, its trailing root dot or its case are
//! written.

/// The `defaultDomain` this service ANNOUNCES for an app, resolved ONCE when the app is
/// created and stored, so every later read reports the same value.
///
/// * a request that addressed the EMULATOR'S OWN endpoint comes from a client that
///   resolves nothing else, so the app's hosting is announced on the port that endpoint
///   answers on, under the loopback name a branch label may be prefixed to
///   ([`loopback_hosting_authority`]);
/// * an app created through any other request on an emulator that IS served on a port is
///   announced on that port the same way, so an app minted by an in-process call of
///   another service announces an address its caller can reach too;
/// * an app created on an emulator with no such address — served on no port at all, or
///   reached on an interface no loopback name covers — is announced on the name
///   [`amplify_zone_default_domain`] builds: the shape `App$defaultDomain` documents,
///   which the hosting edge claims host-style just the same.
pub fn announced_default_domain(
    app_id: &str,
    parts: &::http::request::Parts,
    served_base: &str,
) -> String {
    reached_emulator_endpoint(parts)
        .or_else(|| {
            ::service_common::data_plane_addressing::served_endpoint_authority(served_base)
        })
        .as_deref()
        .and_then(loopback_hosting_authority)
        .unwrap_or_else(|| amplify_zone_default_domain(app_id))
}

/// The `authority` this emulator is reached at, written as an address a branch label can
/// be prefixed to: the LOOPBACK NAME and the port, or `None` when the authority is not the
/// loopback one at all.
///
/// A client composes the site's host as `<branch>.<announced domain>`, so the announced
/// host has to be one a further DNS label resolves under. No resolver answers a name under
/// an IP address, so the address is written as the name RFC 6761 reserves for the loopback
/// interface together with every name under it ([`LOOPBACK_NAME`]) — the interface this
/// emulator's own endpoint answers on. The port is carried over unchanged, because that is
/// what decides which process a client reaches; an endpoint that names no port keeps
/// naming none.
fn loopback_hosting_authority(authority: &str) -> Option<String> {
    let host = ::service_common::data_plane_addressing::host_only(authority);
    if !::emulator_server::is_loopback_host(host) {
        return None;
    }
    Some(
        match ::service_common::data_plane_addressing::addressed_port(Some(authority)) {
            Some(port) => format!("{LOOPBACK_NAME}:{port}"),
            None => LOOPBACK_NAME.to_string(),
        },
    )
}

/// The name of the loopback interface, which RFC 6761 reserves together with every name
/// under it — so a branch's label prefixed to it resolves to the interface this emulator's
/// own endpoint answers on.
const LOOPBACK_NAME: &str = "localhost";

/// The EMULATOR'S OWN endpoint this request reached — an address or a loopback name, which
/// is what a client configured with one local endpoint URL sends — and `None` for a
/// request whose host names a service, which is a client that resolves AWS's own hosts.
///
/// The tree's one reading of that question
/// (`::service_common::data_plane_addressing::reached_emulator_endpoint`), so this service
/// writes no second rule for which host is the emulator's own; it reads the host off the
/// header or off an absolute-form URI, as both carry it.
fn reached_emulator_endpoint(parts: &::http::request::Parts) -> Option<String> {
    ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
}

/// The default domain an app of this id carries under Amplify's OWN hosting zone: the
/// shape `App$defaultDomain` documents, reported by an emulator that is served on no port
/// of its own and so has no address to announce instead.
pub fn amplify_zone_default_domain(app_id: &str) -> String {
    format!("{app_id}.{AMPLIFY_HOSTING_ZONE}")
}

/// The zone Amplify serves an app's default domain under.
const AMPLIFY_HOSTING_ZONE: &str = "amplifyapp.com";

/// The host one branch's site answers at, as a name to compare an addressed host against:
/// the branch's display name, then the default domain of its app, in the one normal form
/// [`hosted_site_host`] reads a host in.
pub fn hosted_branch_host(display_name: &str, default_domain: &str) -> String {
    hosted_site_host(&format!("{display_name}.{default_domain}"))
}

/// A host in the one normal form this edge compares in: no userinfo, no port, no trailing
/// root dot, lowercased — the four ways the same host is written.
///
/// The port is dropped on BOTH sides, so an announced domain that carries this emulator's
/// port is matched by the request that dialled it, and a host is still the same host when
/// a client writes the default port for its scheme or leaves it out.
pub fn hosted_site_host(host: &str) -> String {
    ::service_common::data_plane_addressing::host_only(host)
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request `Parts` addressed to `host`, as the framework hands one to a handler.
    fn addressed(host: Option<&str>, uri: &str) -> ::http::request::Parts {
        let mut request = ::http::Request::builder().uri(uri);
        if let Some(host) = host {
            request = request.header(::http::header::HOST, host);
        }
        request
            .body(())
            .expect("a well-formed request")
            .into_parts()
            .0
    }

    #[test]
    fn an_app_announces_the_port_it_was_created_through_under_the_loopback_name() {
        // The rows span what the reading generalises over: every spelling of the
        // emulator's own endpoint a client is configured with (the IPv4 loopback address,
        // the loopback name, a bracketed IPv6 loopback, an endpoint naming no port), a
        // request carrying its endpoint on the URI alone, a host that names this service
        // (a client that resolves AWS's own hosts) on a served and on an unserved
        // emulator, an emulator served on an interface no loopback name covers, and a
        // request naming no host at all.
        let served = "https://127.0.0.1:39355";
        let routable = "https://10.0.0.5:9297";
        let unserved = ::smithy_server_runtime::UNSERVED_URL_BASE;
        let zone = "dexamplezone.amplifyapp.com";
        let cases: &[(&str, Option<&str>, &str, &str, &str)] = &[
            (
                "the loopback address with a port",
                Some("127.0.0.1:44285"),
                "/apps",
                served,
                "localhost:44285",
            ),
            (
                "the loopback name with a port",
                Some("localhost:9297"),
                "/apps",
                served,
                "localhost:9297",
            ),
            (
                "a bracketed IPv6 loopback",
                Some("[::1]:9297"),
                "/apps",
                served,
                "localhost:9297",
            ),
            (
                "a loopback endpoint naming no port",
                Some("127.0.0.1"),
                "/apps",
                served,
                "localhost",
            ),
            (
                "the endpoint carried on the URI alone",
                None,
                "https://127.0.0.1:44285/apps",
                served,
                "localhost:44285",
            ),
            (
                "this service's own published host on a served emulator",
                Some("amplify.us-east-1.amazonaws.com"),
                "/apps",
                served,
                "localhost:39355",
            ),
            (
                "this service's own published host on an unserved emulator",
                Some("amplify.us-east-1.amazonaws.com"),
                "/apps",
                unserved,
                zone,
            ),
            (
                "an emulator served on an interface no loopback name covers",
                Some("amplify.us-east-1.amazonaws.com"),
                "/apps",
                routable,
                zone,
            ),
            ("no host at all on an unserved emulator", None, "/apps", unserved, zone),
        ];
        for (case, host, uri, base, expected) in cases {
            let announced =
                announced_default_domain("dexamplezone", &addressed(*host, uri), base);
            assert_eq!(
                announced, *expected,
                "case {case:?}: expected {expected:?}, got {announced:?}"
            );
        }
    }

    #[test]
    fn a_branchs_host_and_an_addressed_host_read_the_same() {
        // The rows span the ways the same host is written, and the two shapes an
        // announced domain takes: the endpoint authority (with and without its port on
        // the request) and the name under Amplify's own zone.
        let cases: &[(&str, &str, &str, &str)] = &[
            ("an announced loopback authority", "main", "localhost:44285", "main.localhost:44285"),
            ("the same authority dialled without the port", "main", "localhost:44285", "main.localhost"),
            ("a display name in mixed case", "Main-Two", "localhost:44285", "MAIN-TWO.localhost:44285"),
            ("a name under Amplify's own zone", "main", "d1example.amplifyapp.com", "main.d1example.amplifyapp.com"),
            ("a rooted host", "main", "d1example.amplifyapp.com", "main.d1example.amplifyapp.com."),
            ("a host carrying userinfo", "main", "localhost:44285", "visitor@main.localhost:44285"),
        ];
        for (case, display_name, default_domain, addressed) in cases {
            let branch = hosted_branch_host(display_name, default_domain);
            assert_eq!(
                branch,
                hosted_site_host(addressed),
                "case {case:?}: the branch's host {branch:?} must match the addressed \
                 {addressed:?}"
            );
        }
    }

    #[test]
    fn a_host_naming_another_branch_or_another_app_does_not_read_as_this_one() {
        // The rows span what must NOT match: another branch's label, another app's
        // domain, the announced domain with no branch label at all, and a label that only
        // shares a prefix with the branch's.
        let cases: &[(&str, &str)] = &[
            ("another branch", "other.localhost:44285"),
            ("another app's domain", "main.d2example.amplifyapp.com"),
            ("the domain with no branch label", "localhost:44285"),
            ("a label sharing a prefix", "mainline.localhost:44285"),
        ];
        let branch = hosted_branch_host("main", "localhost:44285");
        for (case, addressed) in cases {
            assert_ne!(
                branch,
                hosted_site_host(addressed),
                "case {case:?}: {addressed:?} must not read as the branch's host {branch:?}"
            );
        }
    }
}
