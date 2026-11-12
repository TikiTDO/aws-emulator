// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHEN `DeleteAsset` decides a request: the ORDER of the
//! denial against the handler's state lookups.
//!
//! The rule: a DataZone permission is held ON a domain, and the service decides a
//! domain-scoped call by authorization on the domain the request path names BEFORE it
//! resolves that domain, the resource inside it, or any documented precondition of the
//! call — so a call scoped to a domain the store serves no record of is refused with
//! `AccessDeniedException` (HTTP 403; `smithy_query(query="operation",
//! operation="DeleteAsset")` binds it beside `ResourceNotFoundException` at 404 and
//! `ConflictException` at 409), never with an existence or a conflict answer. The
//! denial's `message` is one fixed sentence plus the OPERATION NAME of the request, so
//! nothing the caller put in the path appears in it and two different requests to the
//! same operation are refused identically.
//!
//! Why this is asserted for a DELETE in particular: a client treats 404 on a DELETE as
//! "already gone", so reporting a forbidden delete as a miss loses the permissions
//! signal entirely — the caller believes an asset it may not touch has been removed.
//!
//! Each rule is asserted over a TABLE, not one request:
//!
//! * every branch the handler could otherwise reach (an asset stored in another domain,
//!   an asset a stored filter views, an identifier naming nothing anywhere, the shortest
//!   and the longest identifier `AssetIdentifier` admits, a stored id spelled in another
//!   case) crossed with domain identifiers differing in separator, length and content —
//!   all refused identically, which is what makes the denial first;
//! * the same identifiers under the domain that DOES serve them, which take the
//!   operation's own later branches (404, 409 and the 204 that really removes the
//!   asset) — so the 403 above is the authorization short-circuit and not a blanket
//!   answer;
//! * the asset-scoped operations beside this one under one unserved scope, each carrying
//!   its OWN operation name in the same sentence — so the only variable part of the
//!   message is the operation name of the request.
//!
//! The harness (`Emulator`, `Answer`) is the one every suite in this crate drives the
//! service with; the assertions use the HTTP answer only, never `crate::state`.

use crate::harness::Emulator;
use serde_json::{json, Value};

/// The fixed text of the `AccessDeniedException` message a denied call INSIDE a domain
/// carries. SOURCE: the recorded denial, whose message is this sentence followed by the
/// operation name of the request and nothing else.
const DENIAL_PREFIX: &str = "User is not permitted to perform operation: ";

/// `AssetIdentifier`'s modelled maximum length, from its pattern `^[a-zA-Z0-9_-]{1,36}$`
/// (smithy_query shape="AssetIdentifier").
const IDENTIFIER_MAX: usize = 36;

/// The denial `operation` must answer.
fn denial_message(operation: &str) -> String {
    format!("{DENIAL_PREFIX}{operation}")
}

/// Domain identifiers the store holds no domain for, each satisfying `DomainId`'s
/// modelled pattern `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId"), so
/// nothing refuses them before the handler runs. The rows differ along what that pattern
/// leaves open: which separator, the shortest and the longest tail it admits, and a tail
/// mixing the other characters of the class.
fn unserved_domain_ids() -> Vec<(&'static str, String)> {
    vec![
        ("an underscore separator", "dzd_noassetdomain0001".to_string()),
        ("a hyphen separator", "dzd-noassetdomain0002".to_string()),
        ("the shortest tail the pattern admits", "dzd_a".to_string()),
        (
            "the longest tail the pattern admits",
            format!("dzd-{}", "z".repeat(36)),
        ),
        (
            "a tail mixing digits, hyphens and underscores",
            "dzd_9-absent_asset".to_string(),
        ),
    ]
}

/// One served domain holding a project, an asset nothing references, and an asset a
/// stored filter is a view of.
struct Fixture {
    domain: String,
    /// An asset no filter views: the delete that succeeds.
    plain: String,
    /// An asset a stored filter views: the delete the documented precondition refuses.
    filtered: String,
    /// The filter stored on `filtered`.
    filter: String,
}

impl Fixture {
    fn asset_uri(&self, domain: &str, asset: &str) -> String {
        format!("/v2/domains/{domain}/assets/{asset}")
    }
}

/// Build the fixture through the same HTTP surface a client uses.
async fn fixture(dz: &Emulator, domain_name: &str) -> Fixture {
    let domain = dz.available_domain(domain_name).await;
    let project = dz.project(&domain, "denied scope owner").await;
    let assets_uri = format!("/v2/domains/{domain}/assets");

    let plain = dz
        .call(
            "POST",
            &assets_uri,
            Some(json!({
                "name": "plain asset",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        plain.status, 201,
        "creating the plain asset: expected 201, got {} {:?}",
        plain.status, plain.body
    );

    let filtered = dz
        .call(
            "POST",
            &assets_uri,
            Some(json!({
                "name": "filtered asset",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
                "formsInput": [{
                    "formName": "RelationalTableForm",
                    "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                    "content": "{\"columns\":[{\"columnName\":\"order_id\",\"dataType\":\"bigint\"},\
                                 {\"columnName\":\"region\",\"dataType\":\"string\"}]}",
                }],
            })),
        )
        .await;
    assert_eq!(
        filtered.status, 201,
        "creating the filtered asset: expected 201, got {} {:?}",
        filtered.status, filtered.body
    );
    let filtered_id = filtered.s("id");

    let filter = dz
        .call(
            "POST",
            &format!("{assets_uri}/{filtered_id}/filters"),
            Some(json!({
                "name": "denied scope view",
                "configuration": { "columnConfiguration": { "includedColumnNames": ["order_id"] } },
            })),
        )
        .await;
    assert_eq!(
        filter.status, 201,
        "creating the asset filter: expected 201, got {} {:?}",
        filter.status, filter.body
    );

    Fixture {
        domain,
        plain: plain.s("id"),
        filtered: filtered_id,
        filter: filter.s("id"),
    }
}

/// The asset identifiers the sweep addresses: one row per branch the handler would
/// otherwise reach, plus the bounds `AssetIdentifier` admits.
fn swept_identifiers(fx: &Fixture) -> Vec<(&'static str, String)> {
    vec![
        ("an asset stored in another domain", fx.plain.clone()),
        (
            "an asset a stored filter views in another domain",
            fx.filtered.clone(),
        ),
        (
            "an identifier naming nothing in any domain",
            "nosuchasset".to_string(),
        ),
        ("the shortest identifier the pattern admits", "a".to_string()),
        (
            "the longest identifier the pattern admits",
            "b".repeat(IDENTIFIER_MAX),
        ),
        (
            "a stored identifier spelled in another case",
            fx.plain.to_uppercase(),
        ),
    ]
}

#[tokio::test]
async fn a_delete_scoped_to_an_unserved_domain_is_denied_before_any_state_lookup() {
    let dz = Emulator::new();
    let fx = fixture(&dz, "denied-asset-scope-domain").await;

    for (domain_label, unserved) in unserved_domain_ids() {
        for (asset_label, asset) in swept_identifiers(&fx) {
            let case = format!("{asset_label} through {domain_label}");
            let answer = dz
                .call("DELETE", &fx.asset_uri(&unserved, &asset), None)
                .await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected the modelled denial, got {} {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.s("message"),
                denial_message("DeleteAsset"),
                "case {case:?}: the denial must carry the recorded sentence and the \
                 operation name, got {:?}",
                answer.body
            );
            // Nothing from the path is in the answer: the service says nothing about a
            // domain or an asset the caller holds no permission on. The asset half of
            // the check needs an identifier long enough not to occur in the fixed
            // sentence by accident, which the one-character row is not.
            let message = answer.s("message");
            assert!(
                !message.contains(&unserved),
                "case {case:?}: the denial must not echo the requested domain, got {message:?}"
            );
            assert!(
                asset.len() < 8 || !message.contains(&asset),
                "case {case:?}: the denial must not echo the requested asset, got {message:?}"
            );
            assert!(
                answer.body.get("id").is_none() && answer.body.get("status").is_none(),
                "case {case:?}: a denied delete must report no resource, got {:?}",
                answer.body
            );
        }
    }

    // Not one of the denied calls reached the store: both assets, and the filter on the
    // second, are still exactly where the fixture put them.
    for (label, asset) in [("the plain asset", &fx.plain), ("the filtered asset", &fx.filtered)] {
        let survivor = dz
            .call("GET", &fx.asset_uri(&fx.domain, asset), None)
            .await;
        assert_eq!(
            (survivor.status, survivor.s("id").as_str()),
            (200, asset.as_str()),
            "{label} must survive every denied delete, got {} {:?}",
            survivor.status,
            survivor.body
        );
    }
    let kept_filter = dz
        .call(
            "GET",
            &format!(
                "{}/filters/{}",
                fx.asset_uri(&fx.domain, &fx.filtered),
                fx.filter
            ),
            None,
        )
        .await;
    assert_eq!(
        (kept_filter.status, kept_filter.s("id").as_str()),
        (200, fx.filter.as_str()),
        "the stored filter must survive every denied delete, got {} {:?}",
        kept_filter.status,
        kept_filter.body
    );
}

#[tokio::test]
async fn the_same_identifiers_take_their_own_branches_under_the_domain_that_serves_them() {
    let dz = Emulator::new();
    let fx = fixture(&dz, "served-asset-scope-domain").await;

    // The branches the denial above short-circuited, each reachable under the domain
    // that holds the assets: an identifier naming nothing is the operation's own miss,
    // and an asset a filter views is refused by the documented precondition.
    let missing = dz
        .call("DELETE", &fx.asset_uri(&fx.domain, "nosuchasset"), None)
        .await;
    assert_eq!(
        (missing.status, missing.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "an identifier naming no asset: expected the operation's own miss, got {} {:?}",
        missing.status,
        missing.body
    );
    assert!(
        !missing.s("message").starts_with(DENIAL_PREFIX),
        "a miss inside a served domain is not a denial, got {:?}",
        missing.s("message")
    );

    let refused = dz
        .call("DELETE", &fx.asset_uri(&fx.domain, &fx.filtered), None)
        .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (409, "ConflictException"),
        "an asset a stored filter views: expected the modelled conflict, got {} {:?}",
        refused.status,
        refused.body
    );
    let conflict = refused.s("message");
    assert!(
        conflict.contains(&fx.filtered) && conflict.contains(&fx.filter),
        "the conflict must name the asset and the filter that prevented the delete, got \
         {conflict:?}"
    );

    // And the delete that is allowed really removes the asset: the reader answers the
    // miss afterwards, so the 204 came from the store and not from a constant.
    let deleted = dz
        .call("DELETE", &fx.asset_uri(&fx.domain, &fx.plain), None)
        .await;
    assert_eq!(
        deleted.status, 204,
        "an asset nothing references must delete: expected 204, got {} {:?}",
        deleted.status, deleted.body
    );
    assert!(
        deleted.body == Value::Null || deleted.body == json!({}),
        "a 204 must carry no members, got {:?}",
        deleted.body
    );
    let gone = dz
        .call("GET", &fx.asset_uri(&fx.domain, &fx.plain), None)
        .await;
    assert_eq!(
        (gone.status, gone.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "the deleted asset must stop being readable, got {} {:?}",
        gone.status,
        gone.body
    );
}

#[tokio::test]
async fn the_only_variable_part_of_the_denial_is_the_operation_name_of_the_request() {
    let dz = Emulator::new();
    let fx = fixture(&dz, "denied-asset-wording-domain").await;
    let unserved = "dzd_wordingabsent001";

    // The asset-scoped operations beside the delete, all addressing the SAME stored
    // asset through a domain the store does not serve: each denial is the one sentence
    // with its own operation name substituted, and nothing else.
    let calls: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "DeleteAsset",
            "DELETE",
            fx.asset_uri(unserved, &fx.plain),
            None,
        ),
        ("GetAsset", "GET", fx.asset_uri(unserved, &fx.plain), None),
        (
            "ListAssetRevisions",
            "GET",
            format!("{}/revisions", fx.asset_uri(unserved, &fx.plain)),
            None,
        ),
        (
            "CreateAssetRevision",
            "POST",
            format!("{}/revisions", fx.asset_uri(unserved, &fx.plain)),
            Some(json!({ "name": "denied revision" })),
        ),
        (
            "CreateAssetFilter",
            "POST",
            format!("{}/filters", fx.asset_uri(unserved, &fx.plain)),
            Some(json!({
                "name": "denied filter",
                "configuration": { "columnConfiguration": { "includedColumnNames": ["order_id"] } },
            })),
        ),
    ];

    for (operation, method, uri, body) in calls {
        let answer = dz.call(method, &uri, body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (403, "AccessDeniedException"),
            "case {operation:?}: expected the modelled denial, got {} {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.s("message"),
            denial_message(operation),
            "case {operation:?}: the denial must name the operation of the request, got {:?}",
            answer.body
        );
    }
}
