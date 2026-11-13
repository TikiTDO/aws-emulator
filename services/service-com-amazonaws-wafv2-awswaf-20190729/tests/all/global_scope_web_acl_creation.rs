// SPDX-License-Identifier: CC0-1.0

//! WHERE a web ACL of the GLOBAL namespace can be created, end to end over the wire.
//!
//! The rule under test: a `Scope` a create names is the NAMESPACE the new web ACL lands in,
//! not an endpoint the caller has to reach — so `CreateWebACL` names `CLOUDFRONT` from any
//! regional endpoint and is served there, and the web ACL it stores is the global one either
//! way: its ARN carries `global/webacl/<name>/<id>` and the region the global namespace
//! lives in, whichever endpoint minted it. A create still judges the VALUE: a scope the
//! `Scope` enum does not declare is the invalid-parameter refusal from every endpoint.
//!
//! The other half of the same rule is unchanged and asserted here beside it, so the boundary
//! is visible in one place: a call that RESOLVES something in the global namespace is served
//! only at the endpoint that namespace lives at ("API and SDKs - For all calls, use the
//! Region endpoint us-east-1", `Scope`'s own doc), which is what
//! [`crate::refusal_classes`] spans across the scoped reads.
//!
//! The rows differ along what the rule generalises over: the ENDPOINT the create is signed
//! for (four of them, including the one the global namespace lives at), the SCOPE it names
//! (both modelled values, and two values the enum does not declare), and — for the global
//! namespace — the round trip of reading the created web ACL back and finding it in the
//! listing of its own namespace.
//!
//! Every call goes through the same front door a real client uses (the `Waf` harness in
//! [`crate::web_acl_lifecycle`]), so what these tests observe is what an SDK observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{SCOPE_CLOUDFRONT, SCOPE_REGIONAL};

use crate::web_acl_lifecycle::{is_model_uuid, visibility, Waf, CLOUDFRONT_ENDPOINT_REGION};

/// The endpoints a create is signed for: three that serve a region of their own, and the one
/// the global namespace lives at.
const ENDPOINTS: [&str; 4] = [
    "us-west-2",
    "eu-central-1",
    "ap-southeast-2",
    CLOUDFRONT_ENDPOINT_REGION,
];

/// The `ParameterExceptionField` value naming a `Scope`
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists SCOPE_VALUE among
/// its values).
const FIELD_SCOPE_VALUE: &str = "SCOPE_VALUE";

/// The `Reason` WAF carries for a scope it will not accept, as the recorded refusals of that
/// class word it.
const SCOPE_NOT_VALID: &str = "The scope is not valid.";

/// The namespace segment a `Scope`'s ARNs open with: `global` for the scope CloudFront is
/// protected by, `regional` for the other
/// (`arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`).
fn namespace_segment(scope: &str) -> &'static str {
    if scope == SCOPE_CLOUDFRONT {
        "global"
    } else {
        "regional"
    }
}

/// The region a `scope`'s ARNs carry: the global namespace lives at ONE endpoint, so its
/// resources name that region whatever endpoint minted them, while a regional resource names
/// the region it was created in.
fn arn_region(scope: &str, created_from: &str) -> String {
    if scope == SCOPE_CLOUDFRONT {
        CLOUDFRONT_ENDPOINT_REGION.to_string()
    } else {
        created_from.to_string()
    }
}

/// One `CreateWebACL` body: the smallest web ACL the model admits, under `name` and `scope`.
fn create_body(name: &str, scope: &str) -> Value {
    json!({
        "Name": name,
        "Scope": scope,
        "DefaultAction": { "Allow": {} },
        "VisibilityConfig": visibility(name),
    })
}

/// A create names the namespace its web ACL lands in, and is served at every endpoint: for
/// each (endpoint, scope) pair the call answers 200 and the stored web ACL is the one that
/// scope names — the ARN opens with that namespace and carries the region the namespace
/// lives in — and the global ones are readable and listed in their own namespace afterwards.
#[tokio::test]
async fn a_web_acl_is_created_into_the_namespace_its_scope_names_from_any_endpoint() {
    let waf = Waf::new();
    let account = ::engine::EMULATOR_ACCOUNT_ID;

    for endpoint in ENDPOINTS {
        for scope in [SCOPE_CLOUDFRONT, SCOPE_REGIONAL] {
            let case = format!("{scope} from {endpoint}");
            let name = format!("acl-{}-{endpoint}", namespace_segment(scope));
            let (status, body) = waf
                .call_from(endpoint, "CreateWebACL", create_body(&name, scope))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a create names the namespace its web ACL lands in, so it is \
                 served here; got status {status} and body {body}"
            );

            let summary = &body["Summary"];
            let id = summary["Id"].as_str().unwrap_or_default();
            assert!(
                is_model_uuid(id),
                "case {case:?}: expected a minted entity id, got {id:?} in {body}"
            );
            let expected_arn = format!(
                "arn:aws:wafv2:{}:{account}:{}/webacl/{name}/{id}",
                arn_region(scope, endpoint),
                namespace_segment(scope)
            );
            assert_eq!(
                summary["ARN"].as_str(),
                Some(expected_arn.as_str()),
                "case {case:?}: expected the ARN {expected_arn:?}, got {:?}",
                summary["ARN"]
            );

            // The ROUND TRIP: the web ACL the create stored is the one its own namespace
            // answers with, read at the endpoint that namespace lives at.
            let read_from = if scope == SCOPE_CLOUDFRONT {
                CLOUDFRONT_ENDPOINT_REGION
            } else {
                endpoint
            };
            let (read_status, read) = waf
                .call_from(
                    read_from,
                    "GetWebACL",
                    json!({ "Name": &name, "Scope": scope, "Id": id }),
                )
                .await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected GetWebACL to answer 200 at {read_from}, got status \
                 {read_status} and body {read}"
            );
            assert_eq!(
                read["WebACL"]["ARN"].as_str(),
                Some(expected_arn.as_str()),
                "case {case:?}: expected the read to report ARN {expected_arn:?}, got {read}"
            );

            let (list_status, listed) = waf
                .call_from(read_from, "ListWebACLs", json!({ "Scope": scope }))
                .await;
            assert_eq!(
                list_status, 200,
                "case {case:?}: expected ListWebACLs to answer 200 at {read_from}, got status \
                 {list_status} and body {listed}"
            );
            let named: Vec<&Value> = listed["WebACLs"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter(|item| item["Name"].as_str() == Some(name.as_str()))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                named.len(),
                1,
                "case {case:?}: expected the namespace's listing to hold {name:?} exactly \
                 once, got {listed}"
            );
            assert_eq!(
                named[0]["ARN"].as_str(),
                Some(expected_arn.as_str()),
                "case {case:?}: expected the listing to report ARN {expected_arn:?}, got {}",
                named[0]
            );
        }
    }
}

/// A create still judges the scope VALUE from every endpoint: a value the `Scope` enum does
/// not declare names no namespace, so the call is refused wherever it arrives and nothing is
/// stored for it — which is what makes the endpoint condition the only thing the global
/// namespace's create drops.
///
/// `Scope` is a modelled enum, so this refusal is the framework's own, at the request
/// boundary and ahead of the handler (the shape `crate::parameter_validation` spans); the
/// rows here assert that the boundary still refuses such a value on THIS operation, from
/// every endpoint, and that the store is untouched afterwards.
#[tokio::test]
async fn a_create_naming_no_modelled_scope_is_refused_from_every_endpoint() {
    let waf = Waf::new();

    // The rows span the ways a scope value can name no namespace: a value of neither
    // modelled spelling, and a modelled value written in the wrong case. `Scope` is a
    // modelled enum, so the value is refused at the request boundary — the framework's own
    // enum refusal, which `crate::parameter_validation` spans — before any handler runs.
    for scope in ["GLOBAL", "cloudfront"] {
        for endpoint in ENDPOINTS {
            let case = format!("{scope:?} from {endpoint}");
            let name = "acl-unknown-scope";
            let (status, body) = waf
                .call_from(endpoint, "CreateWebACL", create_body(name, scope))
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected a scope outside the enum to be refused, got status \
                 {status} and body {body}"
            );
            let message = body["message"]
                .as_str()
                .or_else(|| body["Message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains(SCOPE_CLOUDFRONT) && message.contains(SCOPE_REGIONAL),
                "case {case}: expected the refusal to name the values the enum declares, got \
                 {body}"
            );
        }
    }

    // Nothing the refused creates named is in either namespace afterwards.
    for scope in [SCOPE_CLOUDFRONT, SCOPE_REGIONAL] {
        let read_from = if scope == SCOPE_CLOUDFRONT {
            CLOUDFRONT_ENDPOINT_REGION
        } else {
            "us-west-2"
        };
        let (status, listed) = waf
            .call_from(read_from, "ListWebACLs", json!({ "Scope": scope }))
            .await;
        assert_eq!(
            status, 200,
            "case {scope}: expected ListWebACLs to answer 200, got status {status} and body \
             {listed}"
        );
        assert_eq!(
            listed["WebACLs"],
            json!([]),
            "case {scope}: a refused create must store nothing, got {listed}"
        );
    }
}

/// The other half of the rule, unchanged: the global namespace is RESOLVED only at the
/// endpoint it lives at. A web ACL created from a regional endpoint is still read there alone
/// — which is what makes the create's answer about the namespace rather than about the
/// endpoint.
#[tokio::test]
async fn the_global_namespace_is_still_resolved_only_at_its_own_endpoint() {
    let waf = Waf::new();
    let name = "acl-created-away-from-its-namespace";
    let created = waf
        .call_from(
            "eu-central-1",
            "CreateWebACL",
            create_body(name, SCOPE_CLOUDFRONT),
        )
        .await;
    assert_eq!(
        created.0, 200,
        "expected the create to be served, got status {} and body {}",
        created.0, created.1
    );
    let id = created.1["Summary"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for endpoint in ENDPOINTS {
        let case = format!("GetWebACL CLOUDFRONT from {endpoint}");
        let (status, body) = waf
            .call_from(
                endpoint,
                "GetWebACL",
                json!({ "Name": name, "Scope": SCOPE_CLOUDFRONT, "Id": &id }),
            )
            .await;
        if endpoint == CLOUDFRONT_ENDPOINT_REGION {
            assert_eq!(
                status, 200,
                "case {case:?}: the namespace is read where it lives, got status {status} and \
                 body {body}"
            );
            assert_eq!(
                body["WebACL"]["Name"].as_str(),
                Some(name),
                "case {case:?}: expected the web ACL {name:?}, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: the namespace is not read here, got status {status} and body \
                 {body}"
            );
            assert_eq!(
                (body["Field"].as_str(), body["Reason"].as_str()),
                (Some(FIELD_SCOPE_VALUE), Some(SCOPE_NOT_VALID)),
                "case {case:?}: expected the scope refusal, got {body}"
            );
        }
    }
}
