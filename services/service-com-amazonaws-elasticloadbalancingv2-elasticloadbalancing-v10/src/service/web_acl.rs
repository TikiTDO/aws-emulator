// SPDX-License-Identifier: CC0-1.0

//! The WAFv2 WEB ACL in front of an Application Load Balancer: how one request to the
//! name the load balancer publishes is decided before the listener's own actions run.
//!
//! A web ACL is put in front of a load balancer by WAFv2's `AssociateWebACL` over the
//! load balancer's own ARN — an Application Load Balancer is the first of the protected
//! resource forms that call documents — and from then on the load balancer's traffic is
//! decided by that web ACL: a request it refuses is answered at the edge and never
//! reaches the listener's action, and a request it allows reaches the listener carrying
//! the headers every `CustomRequestHandling` on the way inserted.
//!
//! The decision itself is WAFv2's own
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]): this module
//! holds no second reading of a rule tree. It does the two things the decision needs and
//! only this service can do — name WHICH protected resource the request addresses (the
//! load balancer's stored ARN, which is the ARN the binding was written under) and read
//! the inbound request into the components a `FieldToMatch` names — and says what the
//! edge answers:
//!
//! * a decision that lets the request through is [`Decision::Unchanged`], or
//!   [`Decision::Allowed`] with the headers to carry onto the request the listener's
//!   rules are evaluated against;
//! * a terminating action that configures its own `CustomResponse` is served BY WAF, with
//!   that response's status, headers and body;
//! * any other terminating action is answered with the status WAF serves it under
//!   (`403` for a `Block`) and no body — a body WAF renders for a blocked client is not
//!   this emulator's to invent, exactly as the sibling statuses this plane answers with
//!   carry none.

use aws_smithy_runtime_api::http::Request as RtRequest;
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge as waf_edge;
use service_com_amazonaws_wafv2_awswaf_20190729::service::logs as waf_logs;

/// The status a terminating action with no `CustomResponse` of its own is answered under
/// when the action names none — `EdgeAction::default_status` answers one for every
/// terminating action, so this is only the fallback for an action that answers nothing.
const EDGE_REFUSAL_STATUS: u16 = 403;

/// What the web ACL in front of a load balancer made of one request.
#[derive(Debug)]
pub enum Decision {
    /// No web ACL is in front of this load balancer, or the one that is allowed the
    /// request and inserted nothing into it: the listener serves the request as it
    /// arrived.
    Unchanged,
    /// The web ACL allowed the request and a `CustomRequestHandling` on the way inserted
    /// these headers, which the listener's rules and its targets are to see.
    Allowed(Vec<(String, String)>),
    /// The web ACL did not let the request through: this is what the edge answers, and
    /// none of the listener's actions is performed.
    Refused(EdgeAnswer),
}

/// The response the EDGE serves for a request the web ACL did not let through.
#[derive(Debug)]
pub struct EdgeAnswer {
    /// The status the refusal is answered under.
    pub status: u16,
    /// The media type of [`Self::body`], when the web ACL's custom response names one.
    pub content_type: Option<String>,
    /// The body the web ACL's custom response serves, empty when it configures none.
    pub body: Vec<u8>,
    /// The headers the web ACL's custom response configures, in that order.
    pub headers: Vec<(String, String)>,
}

/// What the web ACL associated with `load_balancer_arn` decides about `req`, whose path
/// within the load balancer's own endpoint is `uri_path` and whose query string (without
/// its leading `?`) is `query_string`, beside the record that web ACL's own
/// `LoggingConfiguration` logs about the request.
///
/// The record is WAFv2's ([`waf_logs`]); this plane only DELIVERS it, with
/// `waf_logs::deliver`, because the write is a call made from the operation serving the
/// request. `None` when the web ACL logs nothing about this request.
///
/// The two stores are locked one after the other and never at the same time, and the
/// wafv2 side locks nothing of this service's, so the two cannot deadlock: the caller
/// releases this service's store before asking, and `decide` reads its own records under a
/// shared lock, evaluates with the lock released, writes the data plane's own
/// bookkeeping — the sample `GetSampledRequests` draws from — under an exclusive one, and
/// publishes the `AWS/WAFV2` counters of the dimensions that acted once that lock is gone.
pub fn decision<Eng: Send + Sync + 'static>(
    engine: &Eng,
    load_balancer_arn: &str,
    req: &RtRequest,
    uri_path: &str,
    query_string: &str,
) -> (Decision, Option<waf_logs::LoggedRecord>) {
    // The body as it arrived, which is what a rule inspecting `FieldToMatch$Body` reads.
    // A request this plane serves is one the front door handed over whole, so its body is in
    // memory; a body that cannot be read as bytes is reported rather than quietly inspected
    // as an empty one, because an empty body matches nothing a body rule looks for.
    let body = match req.body().bytes() {
        Some(bytes) => bytes.to_vec(),
        None => {
            tracing::warn!(
                load_balancer = %load_balancer_arn,
                "the request's body cannot be read as bytes, so the web acl's body inspection \
                 runs against no body"
            );
            Vec::new()
        }
    };
    let inspected = waf_edge::InspectedRequest::of_http(req, uri_path, query_string, body);
    let Some(decided) = waf_edge::decide(engine, load_balancer_arn, &inspected) else {
        // No web ACL is in front of this load balancer — or the wafv2 service is not
        // mounted in this process at all, so no binding can exist.
        return (Decision::Unchanged, None);
    };
    let logged = decided.log_record.clone();
    if decided.action.is_allow() {
        if decided.inserted_headers.is_empty() {
            return (Decision::Unchanged, logged);
        }
        return (Decision::Allowed(decided.inserted_headers), logged);
    }
    tracing::info!(
        load_balancer = %load_balancer_arn,
        rule = decided.rule_name.as_deref().unwrap_or("<default action>"),
        "the web acl in front of this load balancer does not let the request through"
    );
    // A terminating action that configures its own `CustomResponse` is served BY WAF: the
    // status, the headers and the body are the web ACL's own, and the listener's actions
    // are not consulted, because the load balancer never produced the response.
    let refused = Decision::Refused(match decided.custom_response {
        Some(custom) => EdgeAnswer {
            status: custom.status,
            content_type: custom.content_type,
            body: custom.body,
            headers: custom.headers,
        },
        None => EdgeAnswer {
            status: decided
                .action
                .default_status()
                .unwrap_or(EDGE_REFUSAL_STATUS),
            content_type: None,
            body: Vec::new(),
            headers: Vec::new(),
        },
    });
    (refused, logged)
}
