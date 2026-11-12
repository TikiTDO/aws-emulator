// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::ElastiCache::CacheCluster` has really
//! created that cluster — in Amazon ElastiCache — so the handle the stack publishes for it
//! resolves there and a later call on the cluster is served.
//!
//! The rule under test: **a stack reaches its completed status only once the service that
//! owns each declared resource has created it, with the configuration the template declared,
//! and the handle the stack publishes (`Ref`) is the identifier that service holds the
//! resource under** — so `DescribeCacheClusters` on what the stack published reports the
//! cluster, and `ModifyCacheCluster` on it is accepted. Two crates have to agree for that to
//! hold (CloudFormation makes the call, ElastiCache serves the cluster), so the suite lives
//! here.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! cluster (`ClusterName`) or leaves CloudFormation to generate the name, which engine and
//! how many nodes it declares, whether it declares a port, a parameter group, a maintenance
//! window and tags, and whether a stack output reads the cluster's endpoint through
//! `Fn::GetAtt`.
//!
//! The deletion half is a row of the same rule: deleting the stack deletes the cluster it
//! created, so ElastiCache no longer holds it afterwards.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, element, elements, form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// How long the suite is willing to wait for a stack operation to settle, and how often it
/// looks again. Generous: the stack's own clock-owed transition and the created cluster's
/// provisioning span both have to come due, and the suite waits for the CONDITION (the
/// stack's status) rather than for a fixed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(100);

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_elasticache_amazonelasticachev9::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one query-protocol operation of `service`, at `version`, with these form members.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon ElastiCache operation over its awsQuery wire.
async fn elasticache(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "elasticache", "2015-02-02", action, members).await
}

/// Why a stack's resources failed, as CloudFormation records it on the stack's own events:
/// the reasons quoted in an assertion message, so a stack that did not complete says what
/// the owning service refused rather than only that it rolled back.
async fn failure_reasons(router: &MultiServiceRouter, stack_name: &str) -> Vec<String> {
    let (_, body) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    elements(&body, "ResourceStatusReason")
}

/// The `DescribeStacks` body once the stack rests in a status that is not in progress, or
/// the last one read when the deadline passes. Waits for the CONDITION, looking again until
/// the stack has settled.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        let settled = opt_element(&body, "StackStatus")
            .is_some_and(|stack_status| !stack_status.ends_with("_IN_PROGRESS"));
        if settled || std::time::Instant::now() >= deadline {
            return body;
        }
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Create the stack `stack_name` from `template` and answer its settled `DescribeStacks`
/// body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    settled_stack(router, case, stack_name).await
}

/// The logical id every row declares its cache cluster under.
const CLUSTER_LOGICAL_ID: &str = "Cache";

/// One row of the table: a stack whose only resource is an `AWS::ElastiCache::CacheCluster`.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The `ClusterName` the template states, or `""` for a template that names the cluster
    /// nowhere — CloudFormation generates the name then, and the `Ref` is that name.
    cluster_name: &'static str,
    engine: &'static str,
    nodes: &'static str,
    node_type: &'static str,
    /// Further properties the row declares, as template JSON members (each with its leading
    /// comma), and what ElastiCache must then report for them.
    extra_properties: &'static str,
    reported: &'static [(&'static str, &'static str)],
}

/// The rows: the axis is what the template states about the cluster, so the rule is not
/// keyed to one declaration shape. The engines are the ones a cache cluster runs
/// (`CreateCacheCluster$Engine`: "The name of the cache engine to be used for this cluster.
/// Valid values for this parameter are: memcached | redis"), and the node count spans the
/// single node a Valkey or Redis OSS cluster takes and the several a Memcached one does.
const ROWS: &[Row] = &[
    Row {
        case: "a named Memcached cluster of two nodes, with tags",
        stack_name: "ecc-named-memcached",
        cluster_name: "cfn-ecc-memcached",
        engine: "memcached",
        nodes: "2",
        node_type: "cache.t3.micro",
        extra_properties: r#","Port":11211,"Tags":[{"Key":"team","Value":"platform"}]"#,
        reported: &[("Engine", "memcached"), ("NumCacheNodes", "2")],
    },
    Row {
        case: "an unnamed single-node Redis OSS cluster, with a parameter group and a window",
        stack_name: "ecc-generated-redis",
        cluster_name: "",
        engine: "redis",
        nodes: "1",
        node_type: "cache.m5.large",
        extra_properties: r#","CacheParameterGroupName":"default.redis7","PreferredMaintenanceWindow":"sun:23:00-mon:01:30""#,
        reported: &[
            ("Engine", "redis"),
            ("NumCacheNodes", "1"),
            ("PreferredMaintenanceWindow", "sun:23:00-mon:01:30"),
        ],
    },
    Row {
        case: "a named single-node Valkey cluster with a snapshot retention limit",
        stack_name: "ecc-named-valkey",
        cluster_name: "Cfn-ECC-Valkey",
        engine: "valkey",
        nodes: "1",
        node_type: "cache.t4g.small",
        extra_properties: r#","SnapshotRetentionLimit":3,"AutoMinorVersionUpgrade":true"#,
        reported: &[("Engine", "valkey"), ("SnapshotRetentionLimit", "3")],
    },
];

/// The template one row deploys: the cluster alone, with its `Ref` and the address of its
/// configuration endpoint published as outputs.
fn template_of(row: &Row) -> String {
    let named = if row.cluster_name.is_empty() {
        String::new()
    } else {
        format!(r#","ClusterName":"{}""#, row.cluster_name)
    };
    format!(
        r#"{{"Resources":{{"{CLUSTER_LOGICAL_ID}":{{"Type":"AWS::ElastiCache::CacheCluster","Properties":{{"Engine":"{}","CacheNodeType":"{}","NumCacheNodes":{}{named}{}}}}}}},"Outputs":{{"ClusterRef":{{"Value":{{"Ref":"{CLUSTER_LOGICAL_ID}"}}}},"Endpoint":{{"Value":{{"Fn::GetAtt":["{CLUSTER_LOGICAL_ID}","ConfigurationEndpoint.Address"]}}}}}}}}"#,
        row.engine, row.node_type, row.nodes, row.extra_properties
    )
}

#[tokio::test]
async fn a_completed_stack_has_created_its_cache_cluster_in_elasticache() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let body = create_and_settle(&router, case, row.stack_name, &template_of(row)).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body} with the failure \
             reasons {:?}",
            failure_reasons(&router, row.stack_name).await
        );

        // The handle the stack publishes for the cluster: "Ref returns the name of the cache
        // cluster", which is the `CacheClusterId` every ElastiCache call addresses it by.
        let outputs = outputs_of(&body);
        let cluster_ref = outputs
            .get("ClusterRef")
            .unwrap_or_else(|| panic!("case {case:?}: no ClusterRef output in {body}"))
            .clone();
        if !row.cluster_name.is_empty() {
            assert_eq!(
                cluster_ref, row.cluster_name,
                "case {case:?}: expected the declared ClusterName as the Ref, got \
                 {cluster_ref:?}"
            );
        }

        // THE RULE: the cluster the stack claims to have created is one ElastiCache holds,
        // under exactly that identifier, with the configuration the template declared.
        let (status, reported) = elasticache(
            &router,
            "DescribeCacheClusters",
            &[("CacheClusterId", cluster_ref.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ElastiCache to report the cluster the stack created, got \
             {status} with body {reported}"
        );
        let cluster = block_of(&reported, "CacheCluster")
            .unwrap_or_else(|| panic!("case {case:?}: no CacheCluster in {reported}"));
        assert_eq!(
            opt_element(cluster, "CacheClusterId").as_deref(),
            Some(cluster_ref.to_ascii_lowercase().as_str()),
            "case {case:?}: expected the cluster to be held under the published handle, got \
             {cluster}"
        );
        assert_eq!(
            opt_element(cluster, "CacheClusterStatus").as_deref(),
            Some("available"),
            "case {case:?}: expected a completed stack to leave the cluster available, got \
             {cluster}"
        );
        for (member, value) in row.reported {
            assert_eq!(
                opt_element(cluster, member).as_deref(),
                Some(*value),
                "case {case:?}: expected {member} to be {value:?}, got {cluster}"
            );
        }

        // The endpoint the stack published for the cluster is the one that service reports
        // for it, not a value CloudFormation invented.
        let endpoint = outputs
            .get("Endpoint")
            .unwrap_or_else(|| panic!("case {case:?}: no Endpoint output in {body}"))
            .clone();
        assert_eq!(
            Some(endpoint.as_str()),
            opt_element(cluster, "Address").as_deref(),
            "case {case:?}: expected the published endpoint to be ElastiCache's own, got \
             {cluster}"
        );

        // And the cluster the stack created takes a live call, which is what makes it the
        // account's cluster rather than a record CloudFormation holds alone.
        let (status, modified) = elasticache(
            &router,
            "ModifyCacheCluster",
            &[
                ("CacheClusterId", cluster_ref.as_str()),
                ("ApplyImmediately", "true"),
                ("PreferredMaintenanceWindow", "tue:04:00-tue:05:00"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stack's cluster to take a modify, got {status} with \
             body {modified}"
        );
        assert_eq!(
            element(&modified, "PreferredMaintenanceWindow"),
            "tue:04:00-tue:05:00",
            "case {case:?}: expected the modify to be applied, got {modified}"
        );
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_cache_cluster_it_created() {
    let case = "delete the stack that created the cache cluster";
    let stack_name = "ecc-deleted";
    let router = build_router();

    let row = Row {
        case,
        stack_name,
        cluster_name: "cfn-ecc-deleted",
        engine: "redis",
        nodes: "1",
        node_type: "cache.t3.micro",
        extra_properties: "",
        reported: &[],
    };
    let body = create_and_settle(&router, case, stack_name, &template_of(&row)).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body} with the failure reasons \
         {:?}",
        failure_reasons(&router, stack_name).await
    );
    let (status, reported) = elasticache(
        &router,
        "DescribeCacheClusters",
        &[("CacheClusterId", row.cluster_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the cluster to exist before the delete, got {status} with \
         body {reported}"
    );

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );

    // The cluster the stack created goes with the stack. ElastiCache leaves a deleted
    // cluster `deleting` until the removal its clock owes comes due, so the suite waits for
    // the CONDITION — the cluster being gone — rather than for a fixed time.
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, reported) = elasticache(
            &router,
            "DescribeCacheClusters",
            &[("CacheClusterId", row.cluster_name)],
        )
        .await;
        if status == 404 {
            assert_eq!(
                opt_element(&reported, "Code").as_deref(),
                Some("CacheClusterNotFound"),
                "case {case:?}: expected the modelled not-found code, got {reported}"
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the cluster to be gone with the stack, last read {status} \
             with body {reported}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}
