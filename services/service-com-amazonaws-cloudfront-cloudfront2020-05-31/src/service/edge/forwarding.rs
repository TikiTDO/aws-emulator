// SPDX-License-Identifier: CC0-1.0

//! WHICH parts of a viewer request take part in the cache key, and which of them travel on
//! the origin request: the one reading of a cache behaviour's CACHE POLICY and its ORIGIN
//! REQUEST POLICY.
//!
//! Both policies name the same three parts of a request — headers, cookies and query
//! strings — and both name them with the same vocabulary
//! (`smithy_query(query="shape",shape="CachePolicyQueryStringBehavior")` and its
//! origin-request-policy twin admit `none`, `whitelist`, `allExcept` and `all`; the header
//! behaviours add `allViewer`, `allViewerAndWhitelistCloudFront` and
//! `allViewerExceptHostHeader`). One [`NameSet`] therefore reads every one of them, and the
//! two policies differ only in what the answer is used FOR:
//!
//! * the CACHE POLICY decides the cache key — "the headers, cookies, and query strings that
//!   are included in the cache key" — and, because "the headers, cookies, and query strings
//!   that are included in the cache key are also included in requests that CloudFront sends
//!   to the origin", everything it keys on also reaches the origin;
//! * the ORIGIN REQUEST POLICY decides what reaches the origin BESIDE that — what the cache
//!   key excludes but the origin still sees.
//!
//! So the origin receives the UNION of the two, and a part named by neither is withheld from
//! it. Nothing here touches the viewer's own credential or a hop-by-hop header, which the
//! caller drops whatever a policy says (`super::HEADERS_NEVER_FORWARDED`).

use super::Behavior;
use crate::state::ComAmazonawsCloudfrontCloudfront20200531State;

/// A behaviour that names nothing: no header, no cookie, no query string.
const NONE: &str = "none";

/// A behaviour that names exactly the listed names.
const WHITELIST: &str = "whitelist";

/// A behaviour that names everything the viewer sent.
const ALL: &str = "all";

/// A behaviour that names everything the viewer sent but the listed names.
const ALL_EXCEPT: &str = "allExcept";

/// The header behaviour that names every header the viewer sent.
const ALL_VIEWER: &str = "allViewer";

/// The header behaviour that names every header the viewer sent and, beside them, the
/// CloudFront-generated headers the list carries ([`super::generated_headers`]).
const ALL_VIEWER_AND_CLOUDFRONT: &str = "allViewerAndWhitelistCloudFront";

/// The header behaviour that names every header the viewer sent but `Host`, which the origin
/// request carries as the origin's own host either way.
const ALL_VIEWER_EXCEPT_HOST: &str = "allViewerExceptHostHeader";

/// The one header `allViewerExceptHostHeader` leaves out.
const HOST: &str = "host";

/// The names of one part of a viewer request that a policy's behaviour names.
///
/// The four shapes every one of these behaviours takes, whichever part of the request and
/// whichever policy it was read from.
#[derive(Debug, PartialEq, Eq, Default)]
pub enum NameSet {
    /// `none`, and a behaviour with no policy to read it from: the set is empty.
    #[default]
    None,
    /// `whitelist`: the named ones, and only those.
    Named(Vec<String>),
    /// `all` (`allViewer`, and the `allViewerAndWhitelistCloudFront` spelling): everything
    /// the viewer sent.
    Every,
    /// `allExcept` (and `allViewerExceptHostHeader`, which excepts `Host`): everything the
    /// viewer sent but the named ones.
    EveryExcept(Vec<String>),
}

impl NameSet {
    /// The set `behavior` names over `names`, the list its configuration carries.
    ///
    /// A behaviour this emulator does not know names NOTHING, so an unreadable configuration
    /// withholds rather than forwards.
    pub fn of(behavior: &str, names: Vec<String>) -> Self {
        match behavior {
            WHITELIST => Self::Named(names),
            ALL | ALL_VIEWER | ALL_VIEWER_AND_CLOUDFRONT => Self::Every,
            ALL_EXCEPT => Self::EveryExcept(names),
            ALL_VIEWER_EXCEPT_HOST => Self::EveryExcept(vec![HOST.to_string()]),
            NONE => Self::None,
            // A behaviour this emulator does not know names nothing either.
            _ => Self::None,
        }
    }

    /// Whether this set carries the COOKIE or QUERY-STRING name `name`.
    ///
    /// Matched exactly: a policy names a cookie or a query-string parameter by the name the
    /// viewer spells it with, and those names are case-SENSITIVE — `sessionid` and
    /// `SessionID` are two different parameters. A header name is the other case, and
    /// [`Self::contains_header`] is what reads one.
    pub fn contains(&self, name: &str) -> bool {
        match self {
            Self::None => false,
            Self::Named(named) => named.iter().any(|listed| listed == name),
            Self::Every => true,
            Self::EveryExcept(named) => !named.iter().any(|listed| listed == name),
        }
    }

    /// Whether this set carries the HEADER name `name`, which is matched without regard to
    /// case (RFC 9110: "field names are case-insensitive").
    pub fn contains_header(&self, name: &str) -> bool {
        match self {
            Self::None => false,
            Self::Named(named) => named.iter().any(|listed| listed.eq_ignore_ascii_case(name)),
            Self::Every => true,
            Self::EveryExcept(named) => {
                !named.iter().any(|listed| listed.eq_ignore_ascii_case(name))
            }
        }
    }

    /// The names this set reports on its own, so a part the cache key carries that the
    /// viewer did NOT send still separates that key from one where it was sent.
    ///
    /// Only a `whitelist` has such names: every other behaviour is read off what the viewer
    /// actually sent.
    pub fn named(&self) -> &[String] {
        match self {
            Self::Named(named) => named,
            _ => &[],
        }
    }

    /// Whether this set carries nothing at all, so there is nothing to select.
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// What a behaviour's CACHE POLICY puts in the cache key — and therefore also sends to the
/// origin.
#[derive(Debug, Default)]
pub struct CacheKeyParameters {
    /// `CachePolicyHeadersConfig$HeaderBehavior`.
    pub headers: NameSet,
    /// `CachePolicyCookiesConfig$CookieBehavior`.
    pub cookies: NameSet,
    /// `CachePolicyQueryStringsConfig$QueryStringBehavior`.
    pub query_strings: NameSet,
    /// `ParametersInCacheKeyAndForwardedToOrigin$EnableAcceptEncodingGzip`.
    pub gzip: bool,
    /// `ParametersInCacheKeyAndForwardedToOrigin$EnableAcceptEncodingBrotli`.
    pub brotli: bool,
}

/// What a behaviour's ORIGIN REQUEST POLICY sends to the origin beside the cache key's own
/// parameters.
#[derive(Debug, Default)]
pub struct ForwardedParameters {
    /// The VIEWER headers the policy forwards.
    pub headers: NameSet,
    /// The headers the EDGE generates that the policy forwards
    /// ([`super::generated_headers`]).
    ///
    /// A generated header is not a header the viewer sent, so a behaviour that forwards "all
    /// headers in viewer requests" does not name one: only a policy that LISTS it does —
    /// `whitelist`, or `allViewerAndWhitelistCloudFront`, whose list is CloudFront's own
    /// headers beside every viewer header.
    pub generated_headers: NameSet,
    /// `OriginRequestPolicyCookiesConfig$CookieBehavior`.
    pub cookies: NameSet,
    /// `OriginRequestPolicyQueryStringsConfig$QueryStringBehavior`.
    pub query_strings: NameSet,
}

/// What the behaviour's cache policy keys on.
///
/// A behaviour that names no cache policy, or one whose policy cannot be read, keys on none
/// of them: what such a behaviour forwards is its own legacy `ForwardedValues` member, not a
/// cache policy's.
pub(super) fn cache_key_parameters(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    behavior: &Behavior,
) -> CacheKeyParameters {
    let Some(id) = behavior.cache_policy_id.as_deref() else {
        return CacheKeyParameters::default();
    };
    // Read the way every other read of a cache policy reads one: a policy this account
    // created, and otherwise the one AWS published under that id.
    let Some(policy) = store.readable_cache_policy(id) else {
        return CacheKeyParameters::default();
    };
    let Some(parameters) = policy
        .config
        .parameters_in_cache_key_and_forwarded_to_origin
        .as_ref()
    else {
        return CacheKeyParameters::default();
    };
    CacheKeyParameters {
        headers: NameSet::of(
            &parameters.headers_config.header_behavior,
            items(parameters.headers_config.headers.as_ref().map(|headers| &headers.items)),
        ),
        cookies: NameSet::of(
            &parameters.cookies_config.cookie_behavior,
            items(parameters.cookies_config.cookies.as_ref().map(|cookies| &cookies.items)),
        ),
        query_strings: NameSet::of(
            &parameters.query_strings_config.query_string_behavior,
            items(
                parameters
                    .query_strings_config
                    .query_strings
                    .as_ref()
                    .map(|names| &names.items),
            ),
        ),
        gzip: parameters.enable_accept_encoding_gzip,
        brotli: parameters.enable_accept_encoding_brotli.unwrap_or(false),
    }
}

/// What the behaviour's origin request policy forwards.
///
/// A behaviour that names no origin request policy forwards nothing of its own, which is
/// CloudFront's default: the origin then sees exactly what the cache key carries.
pub(super) fn forwarded_parameters(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    behavior: &Behavior,
) -> ForwardedParameters {
    let Some(id) = behavior.origin_request_policy_id.as_deref() else {
        return ForwardedParameters::default();
    };
    let Some(policy) = store.readable_origin_request_policy(id) else {
        return ForwardedParameters::default();
    };
    let config = &policy.config;
    let header_behavior = config.headers_config.header_behavior.as_str();
    let header_names = items(config.headers_config.headers.as_ref().map(|headers| &headers.items));
    let generated_headers = match header_behavior {
        WHITELIST | ALL_VIEWER_AND_CLOUDFRONT => NameSet::Named(header_names.clone()),
        _ => NameSet::None,
    };
    ForwardedParameters {
        headers: NameSet::of(header_behavior, header_names),
        generated_headers,
        cookies: NameSet::of(
            &config.cookies_config.cookie_behavior,
            items(config.cookies_config.cookies.as_ref().map(|cookies| &cookies.items)),
        ),
        query_strings: NameSet::of(
            &config.query_strings_config.query_string_behavior,
            items(
                config
                    .query_strings_config
                    .query_strings
                    .as_ref()
                    .map(|names| &names.items),
            ),
        ),
    }
}

/// The names a `(Quantity, Items)` wrapper carries, or none when the behaviour names no list
/// at all.
fn items(items: Option<&Option<Vec<String>>>) -> Vec<String> {
    items.cloned().flatten().unwrap_or_default()
}

/// One `name=value` pair of a query string or a `Cookie` header, as the viewer spelled it.
#[derive(Debug, PartialEq, Eq)]
pub struct Pair<'a> {
    /// The name, which is what a policy names.
    pub name: &'a str,
    /// The whole pair, as the viewer sent it — the text that is put back on the wire.
    pub text: &'a str,
}

/// The pairs of a query string, in the order the viewer sent them.
///
/// A parameter with no `=` at all is a name with no value, which is what the whole segment
/// is read as: a policy that names it selects it, and one that does not withholds it.
pub fn query_pairs(query: &str) -> Vec<Pair<'_>> {
    query
        .split('&')
        .filter(|parameter| !parameter.is_empty())
        .map(|parameter| Pair {
            name: parameter.split('=').next().unwrap_or(parameter),
            text: parameter,
        })
        .collect()
}

/// The pairs of a `Cookie` header, in the order the viewer sent them.
///
/// Cookies travel in one header, `name=value` pairs separated by `; ` (RFC 6265), so the
/// header is read into the pairs a policy names one by one.
pub fn cookie_pairs(cookie: &str) -> Vec<Pair<'_>> {
    cookie
        .split(';')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| Pair {
            name: pair.split('=').next().unwrap_or(pair),
            text: pair,
        })
        .collect()
}

/// The query string the ORIGIN sees: the viewer's own parameters that `selected` names, in
/// the order the viewer sent them.
///
/// The order is the viewer's because the origin reads the query string as text, and a
/// reordered one is a different request to anything that parses it positionally.
pub fn selected_query(query: &str, selected: impl Fn(&str) -> bool) -> String {
    let kept: Vec<&str> = query_pairs(query)
        .into_iter()
        .filter(|pair| selected(pair.name))
        .map(|pair| pair.text)
        .collect();
    kept.join("&")
}

/// The `Cookie` header the ORIGIN sees: the viewer's own cookies that `selected` names, in
/// NAME ORDER, or `None` when none of them is named.
///
/// The order is CloudFront's rather than the viewer's: the edge hands the origin the cookies
/// it forwards ordered by name, so the header an origin reads does not depend on the order
/// one viewer happened to send them in. It is the NAME that orders them, not the pair's whole
/// text, so one cookie whose name is a prefix of another's does not sort between that other
/// and its own value.
pub fn selected_cookies(cookie: &str, selected: impl Fn(&str) -> bool) -> Option<String> {
    let mut kept: Vec<Pair<'_>> = cookie_pairs(cookie)
        .into_iter()
        .filter(|pair| selected(pair.name))
        .collect();
    kept.sort_by(|left, right| left.name.cmp(right.name));
    let kept: Vec<&str> = kept.into_iter().map(|pair| pair.text).collect();
    (!kept.is_empty()).then(|| kept.join("; "))
}

/// In which order a set of selected pairs takes part in the cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairOrder {
    /// The order the VIEWER sent them in: two requests that carry the same parameters in a
    /// different order are two objects, which is what a query string the cache key carries
    /// whole does.
    AsSent,
    /// NAME order, which is the order the edge forwards cookies in, so the key does not
    /// depend on the order one viewer sent them in either.
    ByName,
}

/// The part of the CACHE KEY a set of pairs contributes: the pairs the set names, in
/// `order`.
///
/// A pair the viewer sent is rendered `name=value` whatever it spelled — a parameter it sent
/// with no value at all renders with the empty one — and a name the set carries that the
/// viewer did NOT send is rendered as the bare name. So "absent" and "present and empty" are
/// two renderings rather than one, and a request that sent a parameter is never keyed as one
/// that did not.
pub fn keyed_pairs(pairs: &[Pair<'_>], selected: &NameSet, order: PairOrder) -> Vec<String> {
    let mut keyed: Vec<(&str, String)> = pairs
        .iter()
        .filter(|pair| selected.contains(pair.name))
        .map(|pair| {
            let text = if pair.text.contains('=') {
                pair.text.to_string()
            } else {
                format!("{}=", pair.text)
            };
            (pair.name, text)
        })
        .collect();
    // A name the policy keys on that the viewer did not send still separates this key from
    // one where it was sent.
    for name in selected.named() {
        if !pairs.iter().any(|pair| pair.name == name) {
            keyed.push((name, name.to_string()));
        }
    }
    if order == PairOrder::ByName {
        keyed.sort_by(|left, right| left.0.cmp(right.0));
    }
    keyed.into_iter().map(|(_, text)| text).collect()
}

/// What the behaviour's cache policy makes of the viewer's `Accept-Encoding`.
#[derive(Debug, PartialEq, Eq)]
pub enum AcceptEncoding {
    /// Neither compression flag is set, so "CloudFront treats the `Accept-Encoding` header
    /// the same as any other HTTP header in the viewer request": it is in the cache key and
    /// in the origin request only where the policies name it by name.
    Ordinary,
    /// One or both flags are set, so CloudFront "normalizes the value of the viewer's
    /// `Accept-Encoding` header", keys on the normalized value and sends it to the origin.
    ///
    /// The normalized value names ONE coding — the best one this viewer accepts that the
    /// policy enables, Brotli ahead of gzip — so two viewers that accept the same codings in
    /// different words ask for one object rather than two. `None` is the normalization of a
    /// viewer that accepts neither coding: the origin is asked for `identity`, the coding
    /// that names its own bytes, and that is what the key carries.
    Normalized(Option<super::compression::Encoding>),
}

/// The coding an origin request names when the viewer accepts neither coding the policy
/// enables: the "no transformation" coding of RFC 9110, which is what the origin's own bytes
/// are under.
const IDENTITY: &str = "identity";

impl AcceptEncoding {
    /// Whether this cache policy normalizes the header at all, so the viewer's own value
    /// does not travel to the origin.
    pub fn normalizes(&self) -> bool {
        matches!(self, Self::Normalized(_))
    }

    /// The normalized header value, or `None` when this policy normalizes nothing at all.
    pub fn header_value(&self) -> Option<String> {
        match self {
            Self::Ordinary => None,
            Self::Normalized(Some(encoding)) => Some(encoding.token().to_string()),
            Self::Normalized(None) => Some(IDENTITY.to_string()),
        }
    }

    /// The encoding the edge would compress an object with for this viewer: the one the
    /// normalized header names, or `None` when it names none.
    pub fn preferred(&self) -> Option<super::compression::Encoding> {
        match self {
            Self::Ordinary => None,
            Self::Normalized(encoding) => *encoding,
        }
    }
}

/// What this cache policy makes of the `Accept-Encoding` of one viewer request.
///
/// `EnableAcceptEncodingGzip` / `EnableAcceptEncodingBrotli`: "If one or both of these fields
/// is `true` and the viewer request includes the `Accept-Encoding` header", CloudFront
/// normalizes that header, includes the normalized value in the cache key and includes it in
/// the request to the origin. BOTH halves of that condition decide it: a viewer that sent no
/// `Accept-Encoding` at all has nothing to normalize, so its header is the ordinary one it
/// would be under a policy that enables neither coding. Brotli takes precedence over gzip, so
/// a viewer that accepts both is served — and keyed — as a Brotli viewer, and a viewer that
/// accepts neither asks the origin for `identity`.
pub fn accept_encoding(parameters: &CacheKeyParameters, viewer: Option<&str>) -> AcceptEncoding {
    if !parameters.gzip && !parameters.brotli {
        return AcceptEncoding::Ordinary;
    }
    let Some(viewer) = viewer else {
        return AcceptEncoding::Ordinary;
    };
    let enabled = [
        (parameters.brotli, super::compression::Encoding::Brotli),
        (parameters.gzip, super::compression::Encoding::Gzip),
    ];
    let normalized = enabled
        .into_iter()
        .filter(|(enabled, _)| *enabled)
        .map(|(_, encoding)| encoding)
        .find(|encoding| accepts(viewer, encoding.token()));
    AcceptEncoding::Normalized(normalized)
}

/// Whether an `Accept-Encoding` header value accepts `token`.
///
/// The header is a list of codings with optional weights (RFC 9110): a coding the viewer
/// listed with a weight of zero is one it does NOT accept, and `*` accepts every coding the
/// viewer did not refuse that way. A parameter's name is case-insensitive and may carry
/// whitespace around its `=`, so `q=0`, `Q=0` and `q = 0` are one refusal.
fn accepts(header: &str, token: &str) -> bool {
    let mut wildcard = false;
    for coding in header.split(',') {
        let mut parts = coding.split(';').map(str::trim);
        let name = parts.next().unwrap_or_default();
        let refused = parts.any(|parameter| {
            let Some((parameter, weight)) = parameter.split_once('=') else {
                return false;
            };
            parameter.trim().eq_ignore_ascii_case("q")
                && weight.trim().parse::<f32>().is_ok_and(|weight| weight <= 0.0)
        });
        if name.eq_ignore_ascii_case(token) {
            return !refused;
        }
        if name == "*" && !refused {
            wildcard = true;
        }
    }
    wildcard
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::edge::compression::Encoding;

    #[test]
    fn a_behaviour_names_the_set_its_vocabulary_states() {
        // Rows differ along the behaviour — every value the cookie, query-string and header
        // vocabularies admit, plus one no enum carries — and along the name asked about,
        // including one that differs from the listed name only in case: a HEADER name is
        // matched without regard to case, a cookie or query-string name exactly.
        let named = || vec!["sim_fwd".to_string(), "X-Viewer".to_string()];
        // (case, the behaviour, the name asked about, in the set as a cookie, as a header)
        let cases: [(&str, &str, &str, bool, bool); 12] = [
            ("none names nothing", NONE, "sim_fwd", false, false),
            ("whitelist names a listed name", WHITELIST, "sim_fwd", true, true),
            (
                "whitelist names a header whatever its case, and no other cookie",
                WHITELIST,
                "x-viewer",
                false,
                true,
            ),
            ("whitelist names nothing else", WHITELIST, "simunl", false, false),
            ("all names everything", ALL, "simunl", true, true),
            ("allViewer names everything", ALL_VIEWER, "x-anything", true, true),
            (
                "allViewerAndWhitelistCloudFront names everything",
                ALL_VIEWER_AND_CLOUDFRONT,
                "x-anything",
                true,
                true,
            ),
            ("allExcept names everything else", ALL_EXCEPT, "simunl", true, true),
            ("allExcept leaves out a listed name", ALL_EXCEPT, "sim_fwd", false, false),
            (
                "allExcept leaves out a header whatever its case, and keeps the cookie",
                ALL_EXCEPT,
                "x-viewer",
                true,
                false,
            ),
            (
                "allViewerExceptHostHeader leaves out the host",
                ALL_VIEWER_EXCEPT_HOST,
                "host",
                false,
                false,
            ),
            (
                "a behaviour this emulator does not know names nothing",
                "later",
                "sim_fwd",
                false,
                false,
            ),
        ];
        for (case, behavior, name, as_cookie, as_header) in cases {
            let set = NameSet::of(behavior, named());
            let actual = (set.contains(name), set.contains_header(name));
            assert_eq!(
                actual,
                (as_cookie, as_header),
                "case {case:?} ({behavior} / {name}): expected (contains, contains_header) \
                 == {:?}, got {actual:?} (set {set:?})",
                (as_cookie, as_header)
            );
        }
    }

    #[test]
    fn only_the_selected_parameters_travel_and_they_travel_in_the_viewers_order() {
        // Rows differ along the behaviour the selection comes from, the number of parameters
        // the viewer sent, their order, a parameter carrying no value and an empty query.
        let listed = vec!["simfwd".to_string(), "simb".to_string()];
        let cases: [(&str, &str, NameSet, &str); 7] = [
            (
                "an unlisted parameter is withheld",
                "simfwd=1&simunl=leakq&simb=2",
                NameSet::Named(listed.clone()),
                "simfwd=1&simb=2",
            ),
            (
                "the viewer's order is kept",
                "simb=2&simfwd=1",
                NameSet::Named(listed.clone()),
                "simb=2&simfwd=1",
            ),
            (
                "all keeps every parameter",
                "simfwd=1&simunl=leakq",
                NameSet::Every,
                "simfwd=1&simunl=leakq",
            ),
            (
                "allExcept keeps everything else",
                "simnokey=2&a=1",
                NameSet::EveryExcept(vec!["simnokey".to_string()]),
                "a=1",
            ),
            ("none keeps nothing", "a=1&b=2", NameSet::None, ""),
            (
                "a parameter with no value is named by its name",
                "flag&a=1",
                NameSet::Named(vec!["flag".to_string()]),
                "flag",
            ),
            ("an empty query stays empty", "", NameSet::Every, ""),
        ];
        for (case, query, selected, expected) in cases {
            let actual = selected_query(query, |name| selected.contains(name));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the origin to see {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn only_the_selected_cookies_travel_in_name_order() {
        // Rows differ along the behaviour, the number of cookies the viewer sent, their
        // order and their spacing, and along a header carrying none.
        let cases: [(&str, &str, NameSet, Option<&str>); 6] = [
            (
                "an allowlisted cookie travels alone",
                "sim_fwd=c-v1; sim_unl=leakc",
                NameSet::Named(vec!["sim_fwd".to_string()]),
                Some("sim_fwd=c-v1"),
            ),
            (
                "no named cookie leaves no header",
                "sim_unl=leakc",
                NameSet::Named(vec!["sim_fwd".to_string()]),
                None,
            ),
            (
                "the cookies travel in name order, not the viewer's",
                "sim_fwd=c1; sim_b=b1",
                NameSet::Named(vec!["sim_fwd".to_string(), "sim_b".to_string()]),
                Some("sim_b=b1; sim_fwd=c1"),
            ),
            ("all keeps every cookie, in name order", "b=2;a=1", NameSet::Every, Some("a=1; b=2")),
            (
                "allExcept keeps everything else",
                "a=1; drop=me",
                NameSet::EveryExcept(vec!["drop".to_string()]),
                Some("a=1"),
            ),
            ("none keeps nothing", "a=1", NameSet::None, None),
        ];
        for (case, cookie, selected, expected) in cases {
            let actual = selected_cookies(cookie, |name| selected.contains(name));
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected the origin to see {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn the_cache_key_carries_the_named_pairs_in_the_order_that_part_travels_in() {
        // Rows differ along the behaviour, the order the viewer sent the pairs in, the
        // order the part takes part in the key in, whether a pair carries a value at all,
        // and a named parameter the viewer did not send.
        type Row = (&'static str, &'static str, NameSet, PairOrder, Vec<String>);
        let cases: [Row; 8] = [
            (
                "a query string is keyed on as the viewer sent it",
                "z=9&a=1",
                NameSet::Every,
                PairOrder::AsSent,
                vec!["z=9".to_string(), "a=1".to_string()],
            ),
            (
                "cookies are keyed on in name order",
                "z=9;a=1",
                NameSet::Every,
                PairOrder::ByName,
                vec!["a=1".to_string(), "z=9".to_string()],
            ),
            (
                "allExcept keys on everything else",
                "simnokey=2&a=1",
                NameSet::EveryExcept(vec!["simnokey".to_string()]),
                PairOrder::AsSent,
                vec!["a=1".to_string()],
            ),
            (
                "a named parameter the viewer did not send is keyed on as a bare name",
                "a=1",
                NameSet::Named(vec!["a".to_string(), "b".to_string()]),
                PairOrder::AsSent,
                vec!["a=1".to_string(), "b".to_string()],
            ),
            (
                "a parameter sent with an empty value is not the absent one",
                "a=1&b=",
                NameSet::Named(vec!["a".to_string(), "b".to_string()]),
                PairOrder::AsSent,
                vec!["a=1".to_string(), "b=".to_string()],
            ),
            (
                "a parameter sent with no value at all is keyed on with the empty one",
                "flag",
                NameSet::Every,
                PairOrder::AsSent,
                vec!["flag=".to_string()],
            ),
            (
                "an absent cookie is keyed on in name order with the rest",
                "z=9",
                NameSet::Named(vec!["z".to_string(), "a".to_string()]),
                PairOrder::ByName,
                vec!["a".to_string(), "z=9".to_string()],
            ),
            ("none keys on nothing", "a=1", NameSet::None, PairOrder::AsSent, Vec::new()),
        ];
        for (case, query, selected, order, expected) in cases {
            let pairs = if order == PairOrder::ByName {
                cookie_pairs(query)
            } else {
                query_pairs(query)
            };
            let actual = keyed_pairs(&pairs, &selected, order);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the key to carry {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn accept_encoding_is_normalized_to_the_encodings_the_policy_enables() {
        // Rows differ along which flags the policy sets, what the viewer accepts (both, one,
        // neither, a weighted refusal, a wildcard) and whether it sent the header at all.
        let parameters = |gzip: bool, brotli: bool| CacheKeyParameters {
            gzip,
            brotli,
            ..CacheKeyParameters::default()
        };
        let cases: [(&str, bool, bool, Option<&str>, AcceptEncoding); 10] = [
            (
                "neither flag leaves the header ordinary",
                false,
                false,
                Some("br, gzip"),
                AcceptEncoding::Ordinary,
            ),
            (
                "both flags and both encodings: brotli takes precedence",
                true,
                true,
                Some("gzip, deflate, br"),
                AcceptEncoding::Normalized(Some(Encoding::Brotli)),
            ),
            (
                "only brotli enabled",
                false,
                true,
                Some("gzip, br"),
                AcceptEncoding::Normalized(Some(Encoding::Brotli)),
            ),
            (
                "only gzip enabled",
                true,
                false,
                Some("gzip, br"),
                AcceptEncoding::Normalized(Some(Encoding::Gzip)),
            ),
            (
                "the viewer accepts only brotli",
                true,
                true,
                Some("br"),
                AcceptEncoding::Normalized(Some(Encoding::Brotli)),
            ),
            (
                "the viewer accepts neither",
                true,
                true,
                Some("deflate"),
                AcceptEncoding::Normalized(None),
            ),            (
                "a coding the viewer refused with q=0",
                true,
                true,
                Some("br;q=0, gzip"),
                AcceptEncoding::Normalized(Some(Encoding::Gzip)),
            ),
            (
                "a coding the viewer refused with a zero weight, spelled loosely",
                true,
                true,
                Some("br;Q = 0, gzip"),
                AcceptEncoding::Normalized(Some(Encoding::Gzip)),
            ),
            (
                "a wildcard accepts the one that takes precedence",
                true,
                true,
                Some("*"),
                AcceptEncoding::Normalized(Some(Encoding::Brotli)),
            ),
            (
                "a viewer that sent no header has nothing to normalize",
                true,
                true,
                None,
                AcceptEncoding::Ordinary,
            ),
        ];
        for (case, gzip, brotli, viewer, expected) in cases {
            let actual = accept_encoding(&parameters(gzip, brotli), viewer);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn the_normalized_header_value_names_the_coding_the_viewer_gets() {
        // Rows differ along what the normalization kept.
        let cases: [(&str, AcceptEncoding, Option<&str>); 4] = [
            ("an ordinary header has no normalized value", AcceptEncoding::Ordinary, None),
            (
                "brotli",
                AcceptEncoding::Normalized(Some(Encoding::Brotli)),
                Some("br"),
            ),
            ("gzip", AcceptEncoding::Normalized(Some(Encoding::Gzip)), Some("gzip")),
            (
                "neither coding: the origin's own bytes",
                AcceptEncoding::Normalized(None),
                Some(IDENTITY),
            ),
        ];
        for (case, encoding, expected) in cases {
            let actual = encoding.header_value();
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
