// SPDX-License-Identifier: CC0-1.0

//! The WAFv2 WEB ACL in front of a distribution: how one viewer request is decided before
//! the edge reads its cache or reaches an origin.
//!
//! `DistributionConfig$WebACLId` is "a unique identifier that specifies the WAF web ACL, if
//! any, to associate with this distribution. To specify a web ACL created using the latest
//! version of WAF, use the ACL ARN" — and the association is the DISTRIBUTION's own, which
//! is why WAFv2's `AssociateWebACL` documents "for Amazon CloudFront, don't use this call.
//! Instead, use your CloudFront distribution configuration". So this module resolves the web
//! ACL a distribution's stored configuration names and asks WAFv2 about the request:
//! "CloudFront responds to requests either with the requested content or with an HTTP 403
//! status code (Forbidden)", and a refusal is answered at the edge, before the cache is read
//! and with no origin request made.
//!
//! The decision itself is WAFv2's own
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide_against`]): this
//! module holds no second reading of a rule tree. It does the two things only this service
//! can do — name WHICH web ACL the distribution put in front of itself, and read the viewer
//! request into the components a `FieldToMatch` names — and says what the edge answers.

use aws_smithy_runtime_api::http::Request as RtRequest;
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge as waf_edge;
use service_com_amazonaws_wafv2_awswaf_20190729::service::logs as waf_logs;

/// The status a terminating action is answered under when the action names none —
/// `EdgeAction::default_status` answers one for every terminating action, so this is only
/// the fallback, and it is the status CloudFront's own documentation names for a request a
/// web ACL does not let through (`403`).
const EDGE_REFUSAL_STATUS: u16 = 403;

/// What the edge answers for a viewer request the web ACL in front of the distribution did
/// not let through.
#[derive(Debug)]
pub struct Refusal {
    /// The status the refusal is answered under.
    pub status: u16,
    /// The media type of [`Self::body`], when the web ACL's custom response names one.
    pub content_type: Option<String>,
    /// The body the web ACL's custom response serves, empty when it configures none.
    pub body: Vec<u8>,
    /// The headers the web ACL's custom response configures, in that order.
    pub headers: Vec<(String, String)>,
}

/// What the web ACL `web_acl_id` names decides about the viewer request `req`, whose path
/// within the distribution's endpoint is `viewer_path` and whose query string (without its
/// leading `?`) is `query`, beside the record that web ACL's own `LoggingConfiguration`
/// logs about the request.
///
/// A `None` refusal lets the request through: the distribution names no web ACL, names one
/// by an identifier that is no WAFv2 ARN (a WAF Classic ACL id, whose rules this emulator
/// holds none of), names one this emulator does not hold, or the web ACL allowed it.
///
/// The record is WAFv2's ([`waf_logs`]); this service only DELIVERS it, with
/// `waf_logs::deliver`, because the write is a call made from the operation serving the
/// request. `None` when the web ACL logs nothing about this request.
///
/// The two stores are locked one after the other and never at the same time, and the wafv2
/// side locks nothing of this service's, so the two cannot deadlock: the caller releases
/// this service's store before asking, and the decision reads its own records under a
/// shared lock, evaluates with the lock released, writes its data plane's own bookkeeping
/// under an exclusive one, and publishes the `AWS/WAFV2` counters once that lock is gone.
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    web_acl_id: &str,
    req: &RtRequest,
    viewer_path: &str,
    query: &str,
) -> (Option<Refusal>, Option<waf_logs::LoggedRecord>) {
    if !names_a_web_acl_arn(web_acl_id) {
        return (None, None);
    }
    // The body as it arrived, which is what a rule inspecting `FieldToMatch$Body` reads. A
    // viewer request this plane serves is one the front door handed over whole, so its body
    // is in memory; a body that cannot be read as bytes runs against no body rather than
    // being quietly inspected as an empty one.
    let body = req.body().bytes().map(<[u8]>::to_vec).unwrap_or_default();
    let inspected = waf_edge::InspectedRequest::of_http(req, viewer_path, query, body);
    let Some(decided) = waf_edge::decide_against(engine, web_acl_id, &inspected) else {
        return (None, None);
    };
    let logged = decided.log_record.clone();
    if decided.action.is_allow() {
        return (None, logged);
    }
    ::tracing::debug!(
        web_acl = %web_acl_id,
        rule = decided.rule_name.as_deref().unwrap_or("<default action>"),
        "the web acl in front of this distribution does not let the request through"
    );
    // A terminating action that configures its own `CustomResponse` is served BY WAF: the
    // status, the headers and the body are the web ACL's own, and the edge produced none of
    // them.
    let refusal = match decided.custom_response {
        Some(custom) => Refusal {
            status: custom.status,
            content_type: custom.content_type,
            body: custom.body,
            headers: custom.headers,
        },
        None => Refusal {
            status: decided
                .action
                .default_status()
                .unwrap_or(EDGE_REFUSAL_STATUS),
            content_type: None,
            body: Vec::new(),
            headers: Vec::new(),
        },
    };
    (Some(refusal), logged)
}

/// Whether `web_acl_id` names a WAFv2 web ACL by its ARN.
///
/// `WebACLId` carries either form — "to specify a web ACL created using the latest version
/// of WAF, use the ACL ARN … to specify a web ACL created using WAF Classic, use the ACL
/// ID" — and the two are told apart by the shape the value takes: an ARN begins with the
/// `arn:` prefix every Amazon Resource Name does, and a WAF Classic id does not. A
/// configuration that names none states the empty string, which names nothing.
fn names_a_web_acl_arn(web_acl_id: &str) -> bool {
    web_acl_id.starts_with("arn:")
}
