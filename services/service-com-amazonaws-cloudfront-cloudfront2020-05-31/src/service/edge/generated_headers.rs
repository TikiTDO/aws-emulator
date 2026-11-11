// SPDX-License-Identifier: CC0-1.0

//! The request headers the EDGE itself generates, and what a viewer that states one of them
//! gets instead.
//!
//! CloudFront adds headers of its own to an origin request — the `CloudFront-*` family —
//! and they describe the VIEWER'S CONNECTION rather than anything the viewer asked for:
//! where the viewer is (`CloudFront-Viewer-Country`) and which scheme it dialled
//! (`CloudFront-Forwarded-Proto`). Two consequences, and this module owns both:
//!
//! * the edge GENERATES them for every viewer request, including one that sent none of them,
//!   so an origin that reads them reads a value for every request it serves;
//! * a value the VIEWER states for one of them is DROPPED, never relayed: the viewer does
//!   not decide what the connection it opened looks like, so a spoofed `CloudFront-*` header
//!   cannot reach the origin ([`is_generated`] is what the origin-request builder skips on).
//!
//! A generated header reaches the origin only where the behaviour's origin request policy
//! NAMES it ([`super::forwarding::ForwardedParameters::generated_headers`]): a behaviour with
//! no origin request policy, or one whose policy does not list the header, sees none of
//! them.
//!
//! ## Where the values come from
//!
//! The COUNTRY is the viewer's location. CloudFront resolves it from the viewer's address,
//! through the MaxMind GeoIP databases `GeoRestriction` names; nothing of that is reachable
//! here, because every viewer of this emulator dials a loopback address, which is in no
//! country any database places, and no database is consulted off this machine. So the
//! country is read from the one header CloudFront itself states it in when the viewer states
//! a country code there — the addressing choice this plane makes for everything a viewer of
//! a loopback emulator cannot otherwise express — and is the edge's own
//! [`EDGE_COUNTRY`] otherwise. A value that is not a code ISO 3166-1 ASSIGNS to a country is
//! not a country, so it is not read as one either ([`is_country_code`]).
//!
//! The SCHEME is the one the viewer dialled: the request's own URI carries it when the
//! address the viewer reached this emulator at stated it (a distribution's edge listener
//! answers both `http://` and `https://` on one address and states which it served —
//! `crate::state::edge_endpoint`), and `https` is what every other address of this emulator
//! is reached over.

use aws_smithy_runtime_api::http::Request as RtRequest;

/// The prefix of every header name CloudFront generates itself.
///
/// The whole `CloudFront-*` family: whatever the edge does or does not generate, a header
/// under this prefix describes the viewer's connection, so a value a VIEWER states for one is
/// not the edge's and never travels to an origin.
const GENERATED_PREFIX: &str = "cloudfront-";

/// `CloudFront-Viewer-Country`: the viewer's country, as "a two-letter, uppercase country
/// code" — the same `ISO 3166-1 alpha-2` spelling a `GeoRestriction` `Location` carries.
pub const VIEWER_COUNTRY: &str = "cloudfront-viewer-country";

/// `CloudFront-Forwarded-Proto`: the scheme the viewer dialled the edge over.
pub const FORWARDED_PROTO: &str = "cloudfront-forwarded-proto";

/// The country the edge reports for a viewer whose location it is told nothing about.
///
/// A real `ISO 3166-1 alpha-2` code, as every value of this header is: the code of the
/// country the Region CloudFront's own endpoint is served from
/// (`service_common::management_events::PARTITION_LEADER_REGION`, `us-east-1`) is in — the
/// United States. Every viewer of this emulator dials a loopback address, so this is the one
/// location there is to report, and it is the same for every request, which is what an
/// origin comparing two requests' countries reads.
const EDGE_COUNTRY: &str = "US";

/// The scheme a viewer that reached an address of this emulator dialled, unless its request
/// states otherwise: every endpoint this emulator publishes is served over TLS.
const DEFAULT_SCHEME: &str = "https";

/// The scheme a viewer that dialled the plaintext half of an address used.
const PLAINTEXT_SCHEME: &str = "http";

/// Whether `name` is a header name CloudFront generates itself, rather than one a viewer
/// states.
///
/// A header name is case-insensitive (RFC 9110), so the prefix is matched without regard to
/// case: the drop this module documents holds for `CloudFront-Viewer-Country` exactly as it
/// does for `cloudfront-viewer-country`.
pub fn is_generated(name: &str) -> bool {
    name.get(..GENERATED_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(GENERATED_PREFIX))
}

/// The `CloudFront-*` headers the edge generates for one viewer request.
#[derive(Debug, PartialEq, Eq)]
pub struct Generated {
    /// `CloudFront-Viewer-Country`.
    pub country: String,
    /// `CloudFront-Forwarded-Proto`.
    pub scheme: &'static str,
}

impl Generated {
    /// What the edge generates for `req`.
    pub fn of_request(req: &RtRequest) -> Self {
        Self {
            country: viewer_country(req),
            scheme: viewer_scheme(req.uri()),
        }
    }

    /// The generated headers as `(name, value)` pairs, lowercased as an origin request
    /// carries them.
    pub fn headers(&self) -> [(String, String); 2] {
        [
            (VIEWER_COUNTRY.to_string(), self.country.clone()),
            (FORWARDED_PROTO.to_string(), self.scheme.to_string()),
        ]
    }
}

/// The country the edge resolves for one viewer request — the ONE value this request's
/// country has, read by the distribution's geographic restriction and by the header an origin
/// receives alike.
pub fn viewer_country(req: &RtRequest) -> String {
    match req.headers().get(VIEWER_COUNTRY) {
        Some(stated) if is_country_code(stated) => stated.to_ascii_uppercase(),
        // A viewer that stated no country, or stated something that is not a country code.
        _ => EDGE_COUNTRY.to_string(),
    }
}

/// The scheme a viewer dialled, read off the request's own URI.
fn viewer_scheme(uri: &str) -> &'static str {
    match uri.split_once("://") {
        Some((scheme, _rest)) if scheme.eq_ignore_ascii_case(PLAINTEXT_SCHEME) => PLAINTEXT_SCHEME,
        // An absolute URI naming any other scheme, and an origin-form URI, which the
        // emulator's own TLS endpoints are reached with.
        _ => DEFAULT_SCHEME,
    }
}

/// Whether `code` is a code ISO 3166-1 ASSIGNS to a country.
///
/// Two ASCII letters, and not one of the codes the standard RESERVES for user assignment —
/// `AA`, `QM` through `QZ`, `XA` through `XZ` and `ZZ` — which name no country and so cannot
/// be a country a viewer is in.
fn is_country_code(code: &str) -> bool {
    let mut letters = code.chars();
    let (Some(first), Some(second), None) = (letters.next(), letters.next(), letters.next()) else {
        return false;
    };
    if !first.is_ascii_alphabetic() || !second.is_ascii_alphabetic() {
        return false;
    }
    let (first, second) = (
        first.to_ascii_uppercase(),
        second.to_ascii_uppercase(),
    );
    let user_assigned = match first {
        'A' => second == 'A',
        'Q' => ('M'..='Z').contains(&second),
        'X' => second.is_ascii_uppercase(),
        'Z' => second == 'Z',
        _ => false,
    };
    !user_assigned
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

    /// A viewer request carrying `headers`, addressed at `uri`.
    fn request(uri: &str, headers: &[(&str, &str)]) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri(uri.to_string()).expect("a valid URI");
        for (name, value) in headers {
            req.headers_mut()
                .insert((*name).to_string(), (*value).to_string());
        }
        req
    }

    #[test]
    fn the_edge_generates_a_country_and_the_viewers_scheme() {
        // Rows differ along what the viewer stated for the country — nothing, a code, a
        // lower-case code, a code ISO reserves for user assignment, something that is not a
        // code at all — and along the scheme its request was addressed with.
        // (case, the URI it was addressed at, the headers it carried, the country, the
        // scheme)
        type Row = (
            &'static str,
            &'static str,
            &'static [(&'static str, &'static str)],
            &'static str,
            &'static str,
        );
        let cases: [Row; 7] = [
            ("a viewer that stated nothing", "/obj", &[], EDGE_COUNTRY, "https"),
            (
                "a viewer in a country it stated",
                "/obj",
                &[(VIEWER_COUNTRY, "DE")],
                "DE",
                "https",
            ),
            (
                "a country stated in lower case",
                "/obj",
                &[(VIEWER_COUNTRY, "de")],
                "DE",
                "https",
            ),
            (
                "a code ISO reserves for user assignment",
                "/obj",
                &[(VIEWER_COUNTRY, "ZZ")],
                EDGE_COUNTRY,
                "https",
            ),
            (
                "another reserved code",
                "/obj",
                &[(VIEWER_COUNTRY, "XK")],
                EDGE_COUNTRY,
                "https",
            ),
            (
                "a value that is not a country code",
                "/obj",
                &[(VIEWER_COUNTRY, "Elbonia")],
                EDGE_COUNTRY,
                "https",
            ),
            (
                "a viewer that dialled the plaintext half of the address",
                "http://d111.cloudfront.net/obj",
                &[],
                EDGE_COUNTRY,
                "http",
            ),
        ];
        for (case, uri, headers, country, scheme) in cases {
            let generated = Generated::of_request(&request(uri, headers));
            assert_eq!(
                (generated.country.as_str(), generated.scheme),
                (country, scheme),
                "case {case:?}: expected the country {country:?} and the scheme {scheme:?}, \
                 got {generated:?}"
            );
            // Whatever it resolved to, the country is a two-letter uppercase code, which is
            // what an origin reading this header parses.
            assert!(
                generated.country.len() == 2
                    && generated
                        .country
                        .chars()
                        .all(|letter| letter.is_ascii_uppercase()),
                "case {case:?}: expected a two-letter uppercase country code, got {:?}",
                generated.country
            );
        }
    }

    #[test]
    fn every_cloudfront_header_is_one_the_edge_generates() {
        // Rows differ along the header name: the two the edge generates, other members of
        // the family it does not, a member spelled in another case, and headers outside it.
        let cases: [(&str, &str, bool); 8] = [
            ("the viewer's country", VIEWER_COUNTRY, true),
            ("the forwarded scheme", FORWARDED_PROTO, true),
            ("another member of the family", "cloudfront-viewer-address", true),
            ("a member as a viewer would spell it", "CloudFront-Viewer-Country", true),
            ("a member in upper case", "CLOUDFRONT-FORWARDED-PROTO", true),
            ("a viewer header", "accept-language", false),
            ("a header that merely mentions it", "x-cloudfront-hint", false),
            ("the empty name", "", false),
        ];
        for (case, name, expected) in cases {
            let actual = is_generated(name);
            assert_eq!(
                actual, expected,
                "case {case:?} ({name:?}): expected is_generated == {expected}, got {actual}"
            );
        }
    }
}
