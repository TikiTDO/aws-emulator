// SPDX-License-Identifier: CC0-1.0

//! WHAT the `execute-api` data plane REPORTS when IAM refuses an `AWS_IAM` method's invoke:
//! the method ARN the refusal names, and the clause the sentence closes with.
//!
//! THE RULE UNDER TEST: an `execute-api` authorization refusal is `403` with
//! `x-amzn-ErrorType: AccessDeniedException` and the sentence
//! `User: {principal} is not authorized to perform: execute-api:Invoke on resource:
//! {method-arn} {reason}`, where
//!
//! * `{method-arn}` is `arn:aws:execute-api:{region}:{account}:{api-id}/{stage}/{VERB}/{path}`
//!   and `{path}` is the path the CLIENT addressed within the stage — every path parameter
//!   standing for the value the request sent, duplicate slashes collapsed — never the matched
//!   resource's own path TEMPLATE, so two requests to one parameterised method are refused
//!   about two different resources and a grant read back out of the sentence names a path a
//!   client can address;
//! * `{reason}` is the clause IAM closes an authorization failure with, naming the document
//!   class that refused the call and whether its deny was explicit — the only part of the
//!   answer that tells a caller whether a grant would lift the refusal.
//!
//! The rows span what the rule generalises over: the SHAPE of the resource the method is
//! deployed under (the stage root, a literal segment, a literal parent with one path
//! parameter under it, two path parameters, a greedy `{proxy+}`), the SPELLING the client
//! addressed it by (the plain path, a trailing slash, a doubled slash before and inside the
//! path), and the VALUE a path parameter is bound to — two sibling ids of one method. The
//! second case drives the authorization DECISION over the same axis: a grant naming one
//! concrete path admits exactly the request that addressed it, however that request spelled
//! its slashes, and refuses its sibling about the sibling's own ARN; an explicit deny of that
//! one path closes the sentence with its own clause while the sibling is served.
//!
//! The gate is `src/service/invoke/execute_api_authorization.rs` and the ARN is built by
//! `src/service/resource_policy.rs`'s `Call::method_arn`. The sibling
//! `stage_invoke_identity_policy` suite spans the DOCUMENTS the decision is taken from and
//! owns the seeding, the credential scope and the two assertions this suite drives its rows
//! through; what is added here is the resource the refusal is ABOUT, over the paths a client
//! addresses.

use serde_json::{json, Value};

use crate::execute_api_data_plane::{
    deploy, integration_response, invoke, json_template, method_response, put_integration,
    put_method, Invoked,
};
use crate::rest_api_lifecycle::{create_api, resource_under, Gateway};
use crate::stage_invoke_identity_policy::{
    allow, assert_refused, assert_served, credential, given_policy, given_session, served_body,
    session_arn, Entity, Reason, SESSION_KEY_ID,
};

/// The Region the shared control-plane harness creates the api in, and so the Region of the
/// method ARN a refusal reports.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "beta";

/// The IAM action an invoke of a deployed method asks for.
const INVOKE_ACTION: &str = "execute-api:Invoke";

/// The literal resource: a method deployed under a path carrying no parameter at all, where
/// the template and the addressed path are one string.
const LITERAL: &str = "widgets";

/// The literal parent of the parameterised resource, and the parameter under it — the shape a
/// per-object grant is written about.
const COLLECTION: &str = "data-agreements";
const ITEM_TEMPLATE: &str = "{agreementId}";

/// Two sibling values of that parameter: distinct objects of ONE deployed method.
const ITEM: &str = "dua-benchmark-001";
const SIBLING_ITEM: &str = "dua-002";

/// One api carrying every resource shape the rows address, deployed to [`STAGE`].
struct Deployed {
    /// The api id.
    api: String,
}

impl Deployed {
    /// Stand the api up: a `GET` `AWS_IAM` method behind a `MOCK` integration on the stage
    /// root, on a literal resource, on a parameterised item of a literal collection, on two
    /// nested parameters, and on a greedy parameter — then deploy, because what the data
    /// plane serves is read out of the DEPLOYMENT.
    async fn stand_up(gateway: &Gateway, name: &str) -> Self {
        let (api, root) = create_api(gateway, name).await;
        let deployed = Self { api };
        deployed.mock_method(gateway, &root).await;

        let literal = resource_under(gateway, &deployed.api, &root, LITERAL).await;
        deployed.mock_method(gateway, &literal).await;

        let collection = resource_under(gateway, &deployed.api, &root, COLLECTION).await;
        let item = resource_under(gateway, &deployed.api, &collection, ITEM_TEMPLATE).await;
        deployed.mock_method(gateway, &item).await;

        let tenants = resource_under(gateway, &deployed.api, &root, "tenants").await;
        let tenant = resource_under(gateway, &deployed.api, &tenants, "{tenantId}").await;
        let orders = resource_under(gateway, &deployed.api, &tenant, "orders").await;
        let order = resource_under(gateway, &deployed.api, &orders, "{orderId}").await;
        deployed.mock_method(gateway, &order).await;

        let files = resource_under(gateway, &deployed.api, &root, "files").await;
        let any_file = resource_under(gateway, &deployed.api, &files, "{proxy+}").await;
        deployed.mock_method(gateway, &any_file).await;

        deploy(gateway, &deployed.api, STAGE).await;
        deployed
    }

    /// One `GET` method on `resource` authorized by IAM, answered by a `MOCK` integration
    /// whose response template is the body a SERVED request carries.
    async fn mock_method(&self, gateway: &Gateway, resource: &str) {
        put_method(
            gateway,
            &self.api,
            resource,
            "GET",
            json!({ "authorizationType": "AWS_IAM" }),
        )
        .await;
        put_integration(gateway, &self.api, resource, "GET", json!({ "type": "MOCK" })).await;
        method_response(gateway, &self.api, resource, "GET", "200", json!({})).await;
        integration_response(
            gateway,
            &self.api,
            resource,
            "GET",
            "200",
            json!({ "responseTemplates": json_template(&served_body()) }),
        )
        .await;
    }

    /// The ARN of the `GET` this stage serves at `path` within it: the resource the
    /// authorization decision is taken about, and the one a refusal reports. `path` carries
    /// no leading slash — the ARN's own separator is already there — so the stage root is the
    /// empty string.
    fn method_arn(&self, path: &str) -> String {
        format!(
            "arn:aws:execute-api:{REGION}:{account}:{api}/{STAGE}/GET/{path}",
            account = ::engine::emulator_account_id(),
            api = self.api,
        )
    }

    /// One answered invoke of the stage, signed with the seeded session's credential, where
    /// `within_stage` is sent VERBATIM after the stage segment — which is how a row spells a
    /// doubled or a trailing slash.
    async fn invoke_within_stage(&self, gateway: &Gateway, within_stage: &str) -> Invoked {
        let scope = credential(SESSION_KEY_ID);
        invoke(
            gateway,
            &self.api,
            "GET",
            &format!("/{STAGE}{within_stage}"),
            &[("authorization", scope.as_str())],
        )
        .await
    }
}

/// RULE: the refusal names the method ARN of the path the client addressed — parameters bound
/// to the values the request sent, duplicate slashes collapsed, no template text — and closes
/// with the clause naming the document class that refused the call.
#[tokio::test]
async fn the_refusal_names_the_addressed_path_and_closes_with_its_reason() {
    let gateway = Gateway::new();
    let deployed = Deployed::stand_up(&gateway, "denial-sentence-paths").await;
    given_session(&gateway);
    // The role carries no document at all, so every row below falls to IAM's default deny:
    // what each asserts is the RESOURCE the sentence is about.
    given_policy(&gateway, Entity::Role, None);

    // Rows differ along the shape of the deployed resource, the spelling the client addressed
    // it by, and the value a path parameter is bound to.
    let rows: &[(&str, String, String)] = &[
        ("the stage root", String::new(), String::new()),
        ("the stage root with a trailing slash", "/".to_string(), String::new()),
        (
            "a literal resource",
            format!("/{LITERAL}"),
            LITERAL.to_string(),
        ),
        (
            "a literal resource with a trailing slash",
            format!("/{LITERAL}/"),
            LITERAL.to_string(),
        ),
        (
            "a path parameter bound to one id",
            format!("/{COLLECTION}/{ITEM}"),
            format!("{COLLECTION}/{ITEM}"),
        ),
        (
            "the same method addressed at a sibling id",
            format!("/{COLLECTION}/{SIBLING_ITEM}"),
            format!("{COLLECTION}/{SIBLING_ITEM}"),
        ),
        (
            "a doubled slash ahead of the resource",
            format!("//{COLLECTION}/{ITEM}"),
            format!("{COLLECTION}/{ITEM}"),
        ),
        (
            "doubled slashes throughout the path",
            format!("//{COLLECTION}//{ITEM}"),
            format!("{COLLECTION}/{ITEM}"),
        ),
        (
            "two path parameters",
            "/tenants/t-7/orders/o-9".to_string(),
            "tenants/t-7/orders/o-9".to_string(),
        ),
        (
            "a greedy path parameter over several segments",
            "/files/2026/q1/report.csv".to_string(),
            "files/2026/q1/report.csv".to_string(),
        ),
    ];
    for (case, addressed, judged) in rows {
        let answered = deployed.invoke_within_stage(&gateway, addressed).await;
        assert_refused(
            case,
            &answered,
            &session_arn(),
            &deployed.method_arn(judged),
            Reason::NoIdentityGrant,
        );
        // The template's own text is a path no client can address, so no refusal may quote it
        // back: a grant built from this sentence has to name a reachable resource.
        assert!(
            !answered.body.contains(ITEM_TEMPLATE),
            "case {case:?}: expected the addressed path rather than {ITEM_TEMPLATE}, got {:?}",
            answered.body
        );
    }
}

/// RULE: the decision is taken about that same per-request ARN — a grant naming one concrete
/// path admits exactly the requests that addressed it, whatever their slashes, and refuses a
/// sibling id about the sibling's own ARN; an explicit deny of that path refuses it with the
/// explicit-deny clause while the sibling the grant covers is served.
#[tokio::test]
async fn a_grant_on_one_addressed_path_decides_that_path_alone() {
    let gateway = Gateway::new();
    let deployed = Deployed::stand_up(&gateway, "denial-sentence-per-object").await;
    given_session(&gateway);
    let item_arn = deployed.method_arn(&format!("{COLLECTION}/{ITEM}"));
    let sibling_arn = deployed.method_arn(&format!("{COLLECTION}/{SIBLING_ITEM}"));
    // Every path of the stage is granted, with the one item explicitly denied: an explicit
    // deny is a fact about the document, which is the other clause this gate can close with.
    let denied_item: Value = json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": INVOKE_ACTION, "Resource": deployed.method_arn("*") },
            { "Effect": "Deny", "Action": INVOKE_ACTION, "Resource": item_arn },
        ]
    });

    // Rows differ along the document in force, the path addressed and the spelling it was
    // addressed by.
    type Row<'r> = (&'r str, Value, String, Option<(String, Reason)>);
    let rows: Vec<Row<'_>> = vec![
        (
            "the granted id",
            allow(INVOKE_ACTION, &item_arn),
            format!("/{COLLECTION}/{ITEM}"),
            None,
        ),
        (
            "the granted id spelled with doubled slashes",
            allow(INVOKE_ACTION, &item_arn),
            format!("//{COLLECTION}//{ITEM}"),
            None,
        ),
        (
            "a sibling id of the same method",
            allow(INVOKE_ACTION, &item_arn),
            format!("/{COLLECTION}/{SIBLING_ITEM}"),
            Some((sibling_arn.clone(), Reason::NoIdentityGrant)),
        ),
        (
            "the literal resource the grant does not name",
            allow(INVOKE_ACTION, &item_arn),
            format!("/{LITERAL}"),
            Some((deployed.method_arn(LITERAL), Reason::NoIdentityGrant)),
        ),
        (
            "the one id a document otherwise granting everything denies",
            denied_item.clone(),
            format!("/{COLLECTION}/{ITEM}"),
            Some((item_arn.clone(), Reason::IdentityDeny)),
        ),
        (
            "the sibling that same document grants",
            denied_item,
            format!("/{COLLECTION}/{SIBLING_ITEM}"),
            None,
        ),
    ];
    for (case, document, addressed, refused) in &rows {
        given_policy(&gateway, Entity::Role, Some(document));
        let answered = deployed.invoke_within_stage(&gateway, addressed).await;
        match refused {
            None => assert_served(case, &answered),
            Some((arn, reason)) => {
                assert_refused(case, &answered, &session_arn(), arn, *reason);
            }
        }
    }
}
