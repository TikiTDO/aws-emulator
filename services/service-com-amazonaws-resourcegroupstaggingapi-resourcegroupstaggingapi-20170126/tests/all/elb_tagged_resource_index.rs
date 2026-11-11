// SPDX-License-Identifier: CC0-1.0

//! Elastic Load Balancing as an OWNING service of this API: a load balancer, a target
//! group, a listener and a listener rule each hold their own tag set, so that set — not a
//! second copy here — is what `GetResources` reports, what `TagResources` writes and what
//! `UntagResources` removes from, and an ARN that service holds no resource for is this
//! ARN's own `FailedResourcesMap` entry carrying THAT service's refusal.
//!
//! Every test drives the real wire path over ONE engine carrying both services, which is
//! how the emulator runs: `POST /` with an `X-Amz-Target` and an awsJson body for this
//! API, and an awsQuery form body for Elastic Load Balancing.
//!
//! The tables span the class the rule generalises over: which KIND of resource holds the
//! tags (and with it the ARN form and the `NotFound` an absent one earns), the load
//! balancer family the ARN carries, how many tags the create recorded, a tag with an empty
//! value, and resources carrying no tag at all. The target group the issue cited is one
//! row.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10 as owner_elb;
use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and therefore the one both endpoints
/// serve and the one the ARNs Elastic Load Balancing mints belong to.
const REGION: &str = "us-east-1";

/// The SigV4 signing name this API is routed on.
const TAGGING: &str = "tagging";

/// The routing key that addresses the Elastic Load Balancing v2 model directly: the
/// `elasticloadbalancing` endpoint prefix is SHARED with the Classic model and claimed by
/// its front door, so a suite mounting this model alone addresses it by its own key (the
/// `ROUTE_KEY` in that crate's `src/service.rs`).
const ELB: &str = "elasticloadbalancingv2";

/// The API version every awsQuery envelope below names, read from that model's own
/// statement of it rather than restated here.
const ELB_VERSION: &str = owner_elb::service::shared_endpoint::API_VERSION;

/// A router carrying this API and Elastic Load Balancing over ONE engine — the
/// arrangement `core/router` builds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization` header below
    // is present because it is what routes a request to a signing name, not to be
    // verified.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_elb::EmulatedService::new(engine));
    router
}

/// The account id every ARN this emulator mints carries.
fn account() -> &'static str {
    emulator_server::emulator_account_id()
}

/// An Elastic Load Balancing ARN of the form this emulator mints, in the serving Region
/// and account.
fn elb_arn(resource: &str) -> String {
    format!("arn:aws:elasticloadbalancing:{REGION}:{}:{resource}", account())
}

/// The `Authorization` header that routes a request to `signing_name` in [`REGION`].
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0"
    )
}

/// One tagging-API operation, over the awsJson wire this API publishes.
async fn tagging(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let target = format!("ResourceGroupsTaggingAPI_20170126.{operation}");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", &target)
        .header("authorization", authorization(TAGGING))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{target} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// Percent-encode one form component, so an ARN or a path crosses the wire intact.
/// Escaping every non-alphanumeric byte is always a valid form encoding.
///
/// THE ONE COPY in this crate's tests: every suite that posts an awsQuery form body to an
/// owning service builds it through here ([`crate::tagging_api_fidelity`] creates an Auto
/// Scaling group that way).
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(char::from(*byte)),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One awsQuery form field.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// `Action` plus `Version`, the two fields every awsQuery request carries.
fn action(name: &str) -> Vec<(String, String)> {
    vec![p("Action", name), p("Version", ELB_VERSION)]
}

/// One Elastic Load Balancing operation, over the awsQuery wire that model publishes.
async fn elb(router: &MultiServiceRouter, fields: &[(String, String)]) -> (u16, String) {
    let body = fields
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{ELB}.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(ELB))
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One Elastic Load Balancing operation that must succeed.
async fn elb_ok(router: &MultiServiceRouter, fields: &[(String, String)]) -> String {
    let (status, body) = elb(router, fields).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200 from Elastic Load Balancing, got {status} with body {body}",
        fields.first()
    );
    body
}

/// The RAW text inside the first element named `tag` of `xml`, or `None` when it carries
/// none. Raw, so a container element is split into its members before any entity in a
/// member's own character data is read.
///
/// `pub(crate)` for the same reason [`encode`] is: a sibling suite that reads an owning
/// service's awsQuery answer scrapes it the same way rather than carrying a second copy
/// ([`crate::sns_tagged_resource_index`]).
pub(crate) fn element(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// The character data of the first element named `tag`, with the five predefined XML
/// entities read back: the serializer writes `'` as `&apos;`, so a message quoting an
/// identifier crosses the wire escaped and is the same sentence once read. `pub(crate)` as
/// [`element`] is.
pub(crate) fn text(xml: &str, tag: &str) -> Option<String> {
    element(xml, tag).map(|raw| {
        raw.replace("&apos;", "'")
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    })
}

/// The character data of the first element named `tag`, or a panic naming the body.
/// `pub(crate)` as [`element`] is.
pub(crate) fn require(xml: &str, tag: &str) -> String {
    text(xml, tag).unwrap_or_else(|| panic!("no <{tag}> in {xml}"))
}

/// The `Key`/`Value` pairs of the first `<Tags>` list of an Elastic Load Balancing answer.
/// A tag stored with the empty value serializes as an empty element, which reads back as
/// the empty string.
fn elb_tag_pairs(xml: &str) -> BTreeMap<String, String> {
    let Some(tags) = element(xml, "Tags") else {
        return BTreeMap::new();
    };
    tags.split("<member>")
        .skip(1)
        .filter_map(|member| {
            let key = text(member, "Key")?;
            Some((key, text(member, "Value").unwrap_or_default()))
        })
        .collect()
}

/// The tag map Elastic Load Balancing's own `DescribeTags` reports for one ARN.
async fn elb_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let mut form = action("DescribeTags");
    form.push(p("ResourceArns.member.1", arn));
    elb_tag_pairs(&elb_ok(router, &form).await)
}

/// The ARNs of a `GetResources` page, in the order the service returned them.
fn page_arns(body: &Value) -> Vec<String> {
    body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .map(|mapping| {
            mapping["ResourceARN"]
                .as_str()
                .unwrap_or_else(|| panic!("a mapping with no ResourceARN: {mapping}"))
                .to_string()
        })
        .collect()
}

/// The tag map a `GetResources` page reports for one ARN.
fn page_tags(body: &Value, arn: &str) -> BTreeMap<String, String> {
    let mapping = body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .find(|mapping| mapping["ResourceARN"] == json!(arn))
        .unwrap_or_else(|| panic!("{arn} is not in the page: {body}"));
    mapping["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a mapping with no Tags: {mapping}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().expect("Tag.Key").to_string(),
                tag["Value"].as_str().expect("Tag.Value").to_string(),
            )
        })
        .collect()
}

/// `GetResources` restricted to one ARN.
async fn resources_for(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = tagging(router, "GetResources", json!({ "ResourceARNList": [arn] })).await;
    assert_eq!(status, 200, "case {arn}: GetResources body {body}");
    body
}

/// A tag map as the expected `BTreeMap`.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The form fields that put `tags` on a create.
fn tag_fields(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let at = index + 1;
        fields.push(p(&format!("Tags.member.{at}.Key"), key));
        fields.push(p(&format!("Tags.member.{at}.Value"), value));
    }
    fields
}

/// One taggable Elastic Load Balancing resource: how it was created, the ARN it answers
/// to, the resource-type key its ARN carries, and the tags the create recorded.
struct Row {
    case: &'static str,
    arn: String,
    resource_type: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The tags of the whole estate below, one set per kind, so a row's tags identify it.
const LB_TAGS: &[(&str, &str)] = &[("Environment", "dev"), ("Project", "project1")];
const NET_LB_TAGS: &[(&str, &str)] = &[("family", "network")];
const TG_TAGS: &[(&str, &str)] = &[("sim2real", "yes")];
const LISTENER_TAGS: &[(&str, &str)] = &[("tier", "edge"), ("note", "")];
const RULE_TAGS: &[(&str, &str)] = &[("route", "orders")];

/// Create one of every taggable kind, each with the tags its create carried, plus an
/// UNTAGGED target group as the control row — "`GetResources` does not return untagged
/// resources".
///
/// The rows are returned in the order they were created; the untagged target group's ARN
/// comes back beside them.
async fn estate(router: &MultiServiceRouter) -> (Vec<Row>, String) {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", "tagged-alb"));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    form.extend(tag_fields(LB_TAGS));
    let alb = require(&elb_ok(router, &form).await, "LoadBalancerArn");

    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", "tagged-nlb"));
    form.push(p("Type", "network"));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.extend(tag_fields(NET_LB_TAGS));
    let nlb = require(&elb_ok(router, &form).await, "LoadBalancerArn");

    let mut form = action("CreateTargetGroup");
    form.push(p("Name", "tagged-tg"));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("VpcId", "vpc-12345678"));
    form.extend(tag_fields(TG_TAGS));
    let target_group = require(&elb_ok(router, &form).await, "TargetGroupArn");

    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &alb));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &target_group));
    form.extend(tag_fields(LISTENER_TAGS));
    let listener = require(&elb_ok(router, &form).await, "ListenerArn");

    let mut form = action("CreateRule");
    form.push(p("ListenerArn", &listener));
    form.push(p("Priority", "10"));
    form.push(p("Conditions.member.1.Field", "path-pattern"));
    form.push(p("Conditions.member.1.Values.member.1", "/orders/*"));
    form.push(p("Actions.member.1.Type", "forward"));
    form.push(p("Actions.member.1.TargetGroupArn", &target_group));
    form.extend(tag_fields(RULE_TAGS));
    let rule = require(&elb_ok(router, &form).await, "RuleArn");

    let mut form = action("CreateTargetGroup");
    form.push(p("Name", "untagged-tg"));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "8080"));
    form.push(p("VpcId", "vpc-12345678"));
    let untagged = require(&elb_ok(router, &form).await, "TargetGroupArn");

    let rows = vec![
        Row {
            case: "an Application Load Balancer with two tags",
            arn: alb,
            resource_type: "elasticloadbalancing:loadbalancer",
            tags: LB_TAGS,
        },
        Row {
            case: "a Network Load Balancer, whose ARN carries another family",
            arn: nlb,
            resource_type: "elasticloadbalancing:loadbalancer",
            tags: NET_LB_TAGS,
        },
        Row {
            case: "a target group with one tag",
            arn: target_group,
            resource_type: "elasticloadbalancing:targetgroup",
            tags: TG_TAGS,
        },
        Row {
            case: "a listener, one of whose tags has the empty value",
            arn: listener,
            resource_type: "elasticloadbalancing:listener",
            tags: LISTENER_TAGS,
        },
        Row {
            case: "a listener rule",
            arn: rule,
            resource_type: "elasticloadbalancing:listener-rule",
            tags: RULE_TAGS,
        },
    ];
    (rows, untagged)
}

/// The tags an Elastic Load Balancing create recorded are the resource's own, so
/// `GetResources` queried by that resource's ARN reports exactly one mapping carrying
/// every pair the create carried — with no tagging call in between — and that service's
/// own `DescribeTags` reports the same pairs, because there is ONE tag set and both
/// operations read it.
#[tokio::test]
async fn create_time_elb_tags_are_reported_by_get_resources() {
    let router = emulator();
    let (rows, _) = estate(&router).await;
    for row in &rows {
        let want = expected(row.tags);

        let page = resources_for(&router, &row.arn).await;
        let arns = page_arns(&page);
        assert_eq!(
            arns,
            vec![row.arn.clone()],
            "case {}: expected exactly one mapping for {}, got {arns:?}",
            row.case,
            row.arn
        );
        let got = page_tags(&page, &row.arn);
        assert_eq!(
            got, want,
            "case {}: expected GetResources to report {want:?} for {}, got {got:?}",
            row.case, row.arn
        );

        let echoed = elb_tags(&router, &row.arn).await;
        assert_eq!(
            echoed, want,
            "case {}: expected DescribeTags to report {want:?} for {}, got {echoed:?}",
            row.case, row.arn
        );
    }
}

/// `ResourceTypeFilters` select Elastic Load Balancing resources by the
/// `service[:resourceType]` key their ARN carries: the bare service token selects every
/// tagged resource of that service and a qualified token only that kind.
///
/// The same pass pins the two boundaries of the projection: a resource carrying NO tag is
/// not reported at all, and an ARN naming a resource that was never created is not an
/// error, just absent.
#[tokio::test]
async fn resource_type_filters_select_elb_resources_by_kind() {
    let router = emulator();
    let (rows, untagged) = estate(&router).await;

    let arns_of = |kind: &str| {
        let mut found: Vec<String> = rows
            .iter()
            .filter(|row| row.resource_type == kind)
            .map(|row| row.arn.clone())
            .collect();
        found.sort();
        found
    };
    let mut every: Vec<String> = rows.iter().map(|row| row.arn.clone()).collect();
    every.sort();

    let cases: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        ("the bare service token", vec!["elasticloadbalancing"], every),
        (
            "one qualified kind",
            vec!["elasticloadbalancing:targetgroup"],
            arns_of("elasticloadbalancing:targetgroup"),
        ),
        (
            "the kind with two resources",
            vec!["elasticloadbalancing:loadbalancer"],
            arns_of("elasticloadbalancing:loadbalancer"),
        ),
        (
            "two qualified kinds",
            vec![
                "elasticloadbalancing:listener",
                "elasticloadbalancing:listener-rule",
            ],
            {
                let mut both = arns_of("elasticloadbalancing:listener");
                both.extend(arns_of("elasticloadbalancing:listener-rule"));
                both.sort();
                both
            },
        ),
        (
            "a kind nothing tagged",
            vec!["elasticloadbalancing:truststore"],
            Vec::new(),
        ),
    ];
    for (case, filters, want) in cases {
        let (status, body) = tagging(
            &router,
            "GetResources",
            json!({ "ResourceTypeFilters": filters }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: GetResources body {body}");
        let mut got = page_arns(&body);
        got.sort();
        assert_eq!(
            got, want,
            "case {case}: expected ResourceTypeFilters={filters:?} to select {want:?}, got {got:?}"
        );
        assert!(
            !got.contains(&untagged),
            "case {case}: an untagged resource must not be reported, got {got:?}"
        );
    }

    for (case, arn) in [
        ("an untagged target group", untagged.clone()),
        (
            "a target group that was never created",
            elb_arn("targetgroup/ghost/0123456789abcdef"),
        ),
    ] {
        let page = resources_for(&router, &arn).await;
        let got = page_arns(&page);
        assert!(
            got.is_empty(),
            "case {case}: expected {arn} to be absent from the answer, got {got:?}"
        );
    }
}

/// Elastic Load Balancing holds the tag set of every resource this API can tag, so each
/// one has ONE tag home: that service's own record. A tag written through this API is the
/// resource's own tag, which `DescribeTags` reports; the keys the create recorded and the
/// keys this API did not name are left exactly as they were; and an untag removes the key
/// from that same record, which `GetResources` then reports without it.
///
/// The rows differ along the resource kind, so the rule is shown to be per-resource and
/// not per-service.
#[tokio::test]
async fn elb_tags_live_in_elastic_load_balancings_own_records() {
    let router = emulator();
    let (rows, _) = estate(&router).await;
    for row in &rows {
        let (status, body) = tagging(
            &router,
            "TagResources",
            json!({ "ResourceARNList": [&row.arn], "Tags": { "CostCentre": "cc-42" } }),
        )
        .await;
        assert_eq!(status, 200, "case {}: TagResources body {body}", row.case);
        assert_eq!(
            body["FailedResourcesMap"],
            json!({}),
            "case {}: expected no failed resource, got {}",
            row.case,
            body["FailedResourcesMap"]
        );

        let mut want = expected(row.tags);
        want.insert("CostCentre".to_string(), "cc-42".to_string());
        let got = elb_tags(&router, &row.arn).await;
        assert_eq!(
            got, want,
            "case {}: expected DescribeTags to report {want:?} for {}, got {got:?}",
            row.case, row.arn
        );

        let (status, body) = tagging(
            &router,
            "UntagResources",
            json!({ "ResourceARNList": [&row.arn], "TagKeys": ["CostCentre"] }),
        )
        .await;
        assert_eq!(status, 200, "case {}: UntagResources body {body}", row.case);
        let want = expected(row.tags);
        let got = elb_tags(&router, &row.arn).await;
        assert_eq!(
            got, want,
            "case {}: expected the untag to leave {want:?} on {}, got {got:?}",
            row.case, row.arn
        );
        let page = resources_for(&router, &row.arn).await;
        let got = page_tags(&page, &row.arn);
        assert_eq!(
            got, want,
            "case {}: expected GetResources to report {want:?} for {}, got {got:?}",
            row.case, row.arn
        );
    }
}

/// An Elastic Load Balancing ARN this API cannot resolve is answered per RESOURCE, with
/// that service's own refusal: the `NotFound` of the kind the ARN itself declares, under
/// that error's own wire code and with `StatusCode` 400 — and the other ARNs of the same
/// batch are tagged.
///
/// There is ONE wording for each, not a second copy here: the `ErrorMessage` of the
/// per-resource failure is the very message that service's own `AddTags` answers for the
/// same ARN.
///
/// The rows span every taggable kind — including the trust store, which no row of the
/// estate creates — and both load balancer families, whose ARNs differ in the family
/// segment they carry.
#[tokio::test]
async fn an_absent_elb_arn_fails_with_elastic_load_balancings_own_refusal() {
    let router = emulator();
    let (rows, _) = estate(&router).await;
    let present = rows
        .iter()
        .find(|row| row.resource_type == "elasticloadbalancing:targetgroup")
        .expect("the estate holds a target group")
        .arn
        .clone();

    let ghosts: &[(&str, String, &str)] = &[
        (
            "a target group that was never created",
            elb_arn("targetgroup/ghost/0123456789abcdef"),
            "TargetGroupNotFound",
        ),
        (
            "a load balancer that was never created",
            elb_arn("loadbalancer/app/ghost/0123456789abcdef"),
            "LoadBalancerNotFound",
        ),
        (
            "a listener that was never created",
            elb_arn("listener/app/ghost/0123456789abcdef/fedcba9876543210"),
            "ListenerNotFound",
        ),
        (
            "a rule that was never created",
            elb_arn("listener-rule/app/ghost/0123456789abcdef/fedcba9876543210/00000000000000a1"),
            "RuleNotFound",
        ),
        (
            "a trust store that was never created",
            elb_arn("truststore/ghost/0123456789abcdef"),
            "TrustStoreNotFound",
        ),
        (
            "a Network Load Balancer that was never created",
            elb_arn("loadbalancer/net/ghost/0123456789abcdef"),
            "LoadBalancerNotFound",
        ),
    ];

    for (case, arn, want) in ghosts {
        let (status, body) = tagging(
            &router,
            "TagResources",
            json!({ "ResourceARNList": [arn, &present], "Tags": { "Project": "project1" } }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: TagResources body {body}");
        let failure = &body["FailedResourcesMap"][arn.as_str()];
        assert_eq!(
            failure["ErrorCode"],
            json!(want),
            "case {case}: expected {want} for {arn}, got {}",
            body["FailedResourcesMap"]
        );
        assert_eq!(
            failure["StatusCode"],
            json!(400),
            "case {case}: expected StatusCode 400 for {arn}, got {failure}"
        );
        assert_eq!(
            body["FailedResourcesMap"]
                .as_object()
                .map(serde_json::Map::len),
            Some(1),
            "case {case}: the other ARN of the batch must be tagged, got {}",
            body["FailedResourcesMap"]
        );

        // The SAME refusal Elastic Load Balancing's own tag write answers for that ARN:
        // one code and one sentence, composed by the owning service.
        let mut form = action("AddTags");
        form.push(p("ResourceArns.member.1", arn));
        form.push(p("Tags.member.1.Key", "Project"));
        form.push(p("Tags.member.1.Value", "project1"));
        let (status, own) = elb(&router, &form).await;
        assert_eq!(status, 400, "case {case}: AddTags({arn}) body {own}");
        let own_code = require(&own, "Code");
        let own_message = require(&own, "Message");
        assert_eq!(
            own_code, *want,
            "case {case}: expected AddTags({arn}) to answer {want}, got {own_code}"
        );
        assert_eq!(
            failure["ErrorMessage"],
            json!(own_message),
            "case {case}: expected the per-resource failure to carry {own_message:?}, got {failure}"
        );
    }
}
