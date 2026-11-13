// SPDX-License-Identifier: CC0-1.0

//! The CLASSIC load balancer an ECS service's `loadBalancers` entry attaches, and the two
//! requirements that attachment carries beyond the shape of the entry itself.
//!
//! The rule under test, over the whole class rather than one request:
//!
//! > A `loadBalancers` entry of `CreateService` or `UpdateService` whose
//! > `loadBalancerName` is set attaches the service to a CLASSIC load balancer, and is
//! > accepted only when the account holds a Classic load balancer of that name in the
//! > Region the request was signed for AND the task-definition revision the service runs
//! > declares a network mode whose tasks a Classic load balancer can reach — every mode
//! > except `awsvpc`, whose tasks are associated with an elastic network interface of
//! > their own rather than with the container instance a Classic load balancer registers.
//! > Any other entry is refused `400 InvalidParameterException` with nothing persisted or
//! > changed.
//!
//! Both halves come from `CreateService$loadBalancers`'s own doc
//! (`smithy_query(query="doc", shape="CreateServiceRequest", member="loadBalancers")`):
//! "For Classic Load Balancers, this object must contain the load balancer name, the
//! container name, and the container port to access from the load balancer. ... When a task
//! from this service is placed on a container instance, the container instance is
//! registered with the load balancer that's specified here", and "Services with tasks that
//! use the `awsvpc` network mode (for example, those with the Fargate launch type) only
//! support Application Load Balancers and Network Load Balancers. Classic Load Balancers
//! aren't supported."
//!
//! The load balancer is not a value the request carries: it lives in the Classic Elastic
//! Load Balancing service's own store. So every row here creates (and deletes) its load
//! balancer through that service's own API — `Version=2012-06-01` awsQuery on the
//! `elasticloadbalancing` endpoint the two Elastic Load Balancing models share — on the SAME
//! engine the ECS service runs on, and the ECS calls go through the ECS front door: a
//! cross-service READ pinned end to end rather than at a state struct.
//!
//! What the suites span, beyond the one request the rule was found on: whether the account
//! holds the name (held, never created, held in ANOTHER Region, held and then DELETED), the
//! POSITION of the offending entry in a multi-entry list, a list mixing a Classic
//! attachment with an Elastic Load Balancing v2 one, the revision's `networkMode` over the
//! the model's whole `NetworkMode` enum (`bridge`, `host`, `awsvpc`, `none`, the last of
//! which can declare no port mapping at all, so no binding of any kind resolves against it),
//! the `containerPort` the entry names, and the same rule applied by `UpdateService` — to a
//! list the request names and to the list the service already holds when the request moves it
//! onto another revision.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cluster_with_capacity, list, ok, refusal, SIGNED_REGION,
};
use crate::load_balancer_target_groups::{create_target_group, first_text};
use crate::task_lifecycle::container as plain_container;

/// The cluster every service in this suite is created in.
const CLUSTER: &str = "classicattach";

/// The API version a Classic Elastic Load Balancing awsQuery envelope names, from that
/// model's own service version (`2012-06-01`), which is how the shared endpoint resolves a
/// request to the Classic model rather than the v2 one.
const CLASSIC_API_VERSION: &str = "2012-06-01";

/// A Region that is NOT the one the ECS requests are signed for, for the row about a load
/// balancer the account holds somewhere else.
const OTHER_REGION: &str = "us-east-1";

/// A router carrying the ECS service, the CLASSIC Elastic Load Balancing service and the
/// Elastic Load Balancing v2 service over ONE shared engine — the stores this rule reads
/// across, mounted the way the production router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // An `awsvpc` row places its service in a subnet, which a placement is resolved
    // against ([`crate::ecs_wire::seed_awsvpc_subnets`]).
    crate::ecs_wire::seed_awsvpc_subnets(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One Classic Elastic Load Balancing awsQuery call, signed for `region`, returning the
/// status and the XML body.
async fn classic_call(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut form = vec![
        format!("Action={action}"),
        format!("Version={CLASSIC_API_VERSION}"),
    ];
    for (key, value) in params {
        // An `application/x-www-form-urlencoded` value carries only RFC 3986's unreserved
        // set literally; everything else is percent-encoded.
        let value = percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC);
        form.push(format!("{key}={value}"));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://elasticloadbalancing.{region}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{region}\
                 /elasticloadbalancing/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(form.join("&")))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// Create one Classic load balancer named `name` in `region` through that service's OWN API,
/// and assert it reads back there — the premise every accepting row rests on.
///
/// The smallest load balancer that API accepts: a name, one listener and one Availability
/// Zone of the Region ("You must specify at least one Availability Zone").
async fn create_classic_load_balancer(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    case: &str,
) {
    let params = [
        ("LoadBalancerName", name),
        ("Listeners.member.1.Protocol", "HTTP"),
        ("Listeners.member.1.LoadBalancerPort", "80"),
        ("Listeners.member.1.InstancePort", "80"),
        ("AvailabilityZones.member.1", &format!("{region}a")),
    ]
    .iter()
    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
    .collect::<Vec<_>>();
    let (status, body) = classic_call(router, region, "CreateLoadBalancer", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLoadBalancer must succeed for {name} in {region}, got {status} \
         with {body}"
    );
    assert!(
        first_text(&body, "DNSName").is_some(),
        "case {case:?}: CreateLoadBalancer must answer the load balancer's DNS name, got \
         {body}"
    );
    assert!(
        holds_classic_load_balancer(router, region, name).await,
        "case {case:?}: the Classic service must report {name} in {region} after creating it"
    );
}

/// Delete one Classic load balancer through that service's OWN API, and assert it no longer
/// reads back there.
async fn delete_classic_load_balancer(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    case: &str,
) {
    let params = [("LoadBalancerName".to_string(), name.to_string())];
    let (status, body) = classic_call(router, region, "DeleteLoadBalancer", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteLoadBalancer must succeed for {name}, got {status} with {body}"
    );
    assert!(
        !holds_classic_load_balancer(router, region, name).await,
        "case {case:?}: the Classic service must report no {name} in {region} after deleting \
         it"
    );
}

/// Whether the Classic service itself reports a load balancer named `name` in `region`, read
/// through its own `DescribeLoadBalancers` — the association the ECS rule reads, read the way
/// a client would.
async fn holds_classic_load_balancer(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> bool {
    let params = [(
        "LoadBalancerNames.member.1".to_string(),
        name.to_string(),
    )];
    let (status, body) = classic_call(router, region, "DescribeLoadBalancers", &params).await;
    status == 200
        && body.contains(&format!("<LoadBalancerName>{name}</LoadBalancerName>"))
}

/// One container definition declaring `ports` as its container ports.
fn container(name: &str, ports: &[i64]) -> Value {
    let mut definition = plain_container(name, "public.ecr.aws/nginx/nginx:latest");
    definition["portMappings"] = json!(ports
        .iter()
        .map(|port| json!({ "containerPort": port, "protocol": "tcp" }))
        .collect::<Vec<_>>());
    definition
}

/// Register one revision of `family` whose single container declares `ports`.
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    network_mode: &str,
    ports: &[i64],
    case: &str,
) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": network_mode,
            "containerDefinitions": [container("app", ports)],
        }),
        case,
    )
    .await;
}

/// One `loadBalancers` entry attaching a CLASSIC load balancer.
fn classic_binding(load_balancer: &str, container_port: i64) -> Value {
    json!({
        "loadBalancerName": load_balancer,
        "containerName": "app",
        "containerPort": container_port,
    })
}

/// One `loadBalancers` entry attaching an Elastic Load Balancing v2 target group.
fn target_group_binding(target_group: &str, container_port: i64) -> Value {
    json!({
        "targetGroupArn": target_group,
        "containerName": "app",
        "containerPort": container_port,
    })
}

/// Whether `ListServices` reports the service — what proves a refused create stored nothing,
/// and an accepted one stored something.
async fn is_listed(router: &MultiServiceRouter, name: &str, case: &str) -> bool {
    let body = ok(router, "ListServices", json!({ "cluster": CLUSTER }), case).await;
    list(&body, "serviceArns", case).iter().any(|arn| {
        arn.as_str()
            .is_some_and(|arn| arn.ends_with(&format!("/{name}")))
    })
}

/// The `loadBalancers` the stored service holds, read back through `DescribeServices`: the
/// round trip an accepted attachment has to make.
async fn stored_bindings(router: &MultiServiceRouter, name: &str, case: &str) -> Vec<Value> {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [name] }),
        case,
    )
    .await;
    let services = list(&body, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected DescribeServices to report the one service, got {body}"
    );
    list(&services[0], "loadBalancers", case)
}

/// The `taskDefinition` the stored service runs, read back through `DescribeServices`.
async fn stored_task_definition(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [name] }),
        case,
    )
    .await;
    let services = list(&body, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected DescribeServices to report the one service, got {body}"
    );
    services[0]["taskDefinition"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a taskDefinition, got {body}"))
        .to_string()
}

/// The sentence a `loadBalancerName` naming no Classic load balancer of the account earns.
fn not_found(name: &str) -> String {
    format!("The load balancer with loadBalancerName {name} does not exist")
}

/// The sentence a Classic attachment paired with an `awsvpc` revision earns.
fn incompatible(name: &str) -> String {
    format!(
        "The provided load balancer {name} is a Classic Load Balancer, which is incompatible \
         with the awsvpc network mode specified in the task definition."
    )
}

#[tokio::test]
async fn a_classic_attachment_must_name_a_load_balancer_the_account_holds() {
    let router = emulator();
    cluster_with_capacity(&router, CLUSTER, "setup").await;
    register(&router, "classicbridge", "bridge", &[80, 8080], "setup").await;

    // The load balancers of the account, each created through the Classic API: two in the
    // Region the ECS requests are signed for, one in ANOTHER Region, and one created there
    // and then deleted again.
    create_classic_load_balancer(&router, SIGNED_REGION, "ecs-classic-one", "setup").await;
    create_classic_load_balancer(&router, SIGNED_REGION, "ecs-classic-two", "setup").await;
    create_classic_load_balancer(&router, OTHER_REGION, "ecs-classic-elsewhere", "setup").await;
    create_classic_load_balancer(&router, SIGNED_REGION, "ecs-classic-gone", "setup").await;
    delete_classic_load_balancer(&router, SIGNED_REGION, "ecs-classic-gone", "setup").await;

    /// One row: the case, the `loadBalancerName` the entry carries, the `containerPort` it
    /// names, and the refusal AWS answers (`None` when the attachment resolves).
    type Row = (&'static str, &'static str, i64, bool);
    let rows: [Row; 5] = [
        ("a load balancer the account holds", "ecs-classic-one", 80, true),
        (
            "another held load balancer, on the container's second port",
            "ecs-classic-two",
            8080,
            true,
        ),
        ("a name no load balancer was ever created for", "ecs-classic-missing", 80, false),
        ("a load balancer held in ANOTHER Region", "ecs-classic-elsewhere", 80, false),
        ("a load balancer that was deleted again", "ecs-classic-gone", 80, false),
    ];

    for (index, (case, load_balancer, port, allowed)) in rows.iter().enumerate() {
        let service_name = format!("classicservice{index}");
        let request = json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": "classicbridge",
            "desiredCount": 0,
            "loadBalancers": [classic_binding(load_balancer, *port)],
        });

        if *allowed {
            ok(&router, "CreateService", request, case).await;
            assert_eq!(
                stored_bindings(&router, &service_name, case).await,
                vec![classic_binding(load_balancer, *port)],
                "case {case:?}: the accepted attachment must round-trip through \
                 DescribeServices"
            );
            continue;
        }

        let (code, message) = refusal(&router, "CreateService", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message,
            not_found(load_balancer),
            "case {case:?}: unexpected refusal message"
        );
        assert!(
            !is_listed(&router, &service_name, case).await,
            "case {case:?}: a refused create must store no service"
        );
    }

    // The whole LIST is judged, not just its head: the offending entry is refused wherever
    // it sits, and is the one the message names.
    for (position, case) in [
        (0usize, "the FIRST of two attachments naming no load balancer"),
        (1usize, "the SECOND of two attachments naming no load balancer"),
    ] {
        let service_name = format!("classiclist{position}");
        let mut entries = vec![
            classic_binding("ecs-classic-one", 80),
            classic_binding("ecs-classic-two", 80),
        ];
        entries[position] = classic_binding("ecs-classic-absent", 80);
        let (code, message) = refusal(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": service_name,
                "taskDefinition": "classicbridge",
                "desiredCount": 0,
                "loadBalancers": entries,
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message,
            not_found("ecs-classic-absent"),
            "case {case:?}: unexpected refusal message"
        );
        assert!(
            !is_listed(&router, &service_name, case).await,
            "case {case:?}: a refused create must store no service"
        );
    }

    // A list may mix the two KINDS of attachment: an Elastic Load Balancing v2 target group
    // beside a Classic load balancer, each resolved in its own service's store.
    let case = "a v2 target group beside a Classic load balancer";
    let group = create_target_group(&router, "classic-mixed-tg", "instance", case).await;
    let entries = vec![
        target_group_binding(&group, 80),
        classic_binding("ecs-classic-one", 8080),
    ];
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "classicmixed",
            "taskDefinition": "classicbridge",
            "desiredCount": 0,
            "loadBalancers": entries.clone(),
        }),
        case,
    )
    .await;
    assert_eq!(
        stored_bindings(&router, "classicmixed", case).await,
        entries,
        "case {case:?}: both attachments must round-trip through DescribeServices"
    );
}

#[tokio::test]
async fn a_classic_attachment_is_refused_for_an_awsvpc_revision() {
    let router = emulator();
    cluster_with_capacity(&router, CLUSTER, "setup").await;
    create_classic_load_balancer(&router, SIGNED_REGION, "ecs-mode-classic", "setup").await;

    // One revision per network mode a port mapping can be declared under: the model's
    // `NetworkMode` enum is `bridge`, `host`, `awsvpc`, `none`; the `none` mode "cannot
    // specify port mappings in your container definitions", so it is taken on its own below
    // rather than as a row of this table.
    /// One row: the case, the revision's `networkMode`, its family, and whether AWS accepts a
    /// Classic attachment to a service running it.
    type Row = (&'static str, &'static str, &'static str, bool);
    let rows: [Row; 3] = [
        ("the bridge network mode", "bridge", "modebridge", true),
        ("the host network mode", "host", "modehost", true),
        ("the awsvpc network mode", "awsvpc", "modeawsvpc", false),
    ];

    for (index, (case, mode, family, allowed)) in rows.iter().enumerate() {
        register(&router, family, mode, &[80], case).await;
        let service_name = format!("modeservice{index}");
        let mut request = json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": family,
            "desiredCount": 0,
            "loadBalancers": [classic_binding("ecs-mode-classic", 80)],
        });
        if *mode == "awsvpc" {
            request["networkConfiguration"] = awsvpc_network_configuration();
        }

        if *allowed {
            ok(&router, "CreateService", request, case).await;
            assert_eq!(
                stored_bindings(&router, &service_name, case).await,
                vec![classic_binding("ecs-mode-classic", 80)],
                "case {case:?}: the accepted attachment must round-trip through \
                 DescribeServices"
            );
            continue;
        }

        let (code, message) = refusal(&router, "CreateService", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message,
            incompatible("ecs-mode-classic"),
            "case {case:?}: unexpected refusal message"
        );
        assert!(
            !is_listed(&router, &service_name, case).await,
            "case {case:?}: a refused create must store no service"
        );
    }

    // An `awsvpc` service naming a load balancer the account does not hold is refused for
    // the NAME first: an attachment that resolves to nothing is reported as that, whatever
    // mode the revision declares.
    let case = "an awsvpc revision and a load balancer that does not exist";
    let (code, message) = refusal(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "modeneither",
            "taskDefinition": "modeawsvpc",
            "desiredCount": 0,
            "networkConfiguration": awsvpc_network_configuration(),
            "loadBalancers": [classic_binding("ecs-mode-absent", 80)],
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected an InvalidParameterException, got {code:?} with {message:?}"
    );
    assert_eq!(
        message,
        not_found("ecs-mode-absent"),
        "case {case:?}: unexpected refusal message"
    );
    assert!(
        !is_listed(&router, "modeneither", case).await,
        "case {case:?}: a refused create must store no service"
    );

    // And the `none` network mode, which is the mode no row above covers: "If the network
    // mode is set to none, you cannot specify port mappings in your container definitions",
    // so such a revision publishes no container port at all and the attachment is refused for
    // the port it names — a load-balanced service on a `none` revision is a refusal by
    // itself, whichever kind of load balancer it attaches.
    let case = "the none network mode, which can publish no container port";
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "modenone",
            "networkMode": "none",
            "containerDefinitions": [plain_container("app", "public.ecr.aws/nginx/nginx:latest")],
        }),
        case,
    )
    .await;
    let (code, message) = refusal(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "modenoneservice",
            "taskDefinition": "modenone",
            "desiredCount": 0,
            "loadBalancers": [classic_binding("ecs-mode-classic", 80)],
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected an InvalidParameterException, got {code:?} with {message:?}"
    );
    assert_eq!(
        message, "The container app did not have a container port 80 defined.",
        "case {case:?}: unexpected refusal message"
    );
    assert!(
        !is_listed(&router, "modenoneservice", case).await,
        "case {case:?}: a refused create must store no service"
    );
}

#[tokio::test]
async fn an_update_is_held_to_the_same_classic_attachment_rules() {
    let router = emulator();
    cluster_with_capacity(&router, CLUSTER, "setup").await;
    create_classic_load_balancer(&router, SIGNED_REGION, "ecs-update-first", "setup").await;
    create_classic_load_balancer(&router, SIGNED_REGION, "ecs-update-second", "setup").await;
    // Two `bridge` revisions of one family, and an `awsvpc` revision of another: an update
    // may move the service between revisions, and only the `awsvpc` one cannot be reached
    // through a Classic load balancer.
    register(&router, "updatebridge", "bridge", &[80], "setup").await;
    register(&router, "updatebridge", "bridge", &[80, 8080], "setup").await;
    register(&router, "updateawsvpc", "awsvpc", &[80], "setup").await;

    let service_name = "classicupdated";
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": "updatebridge:1",
            "desiredCount": 0,
            "loadBalancers": [classic_binding("ecs-update-first", 80)],
        }),
        "setup",
    )
    .await;

    // A list the UPDATE names is resolved exactly as the create's was: a held name is
    // accepted and replaces the stored attachment, and a name the account holds nothing for
    // is refused with the service left as it was.
    let case = "an update onto another held load balancer";
    ok(
        &router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": service_name,
            "loadBalancers": [classic_binding("ecs-update-second", 80)],
        }),
        case,
    )
    .await;
    assert_eq!(
        stored_bindings(&router, service_name, case).await,
        vec![classic_binding("ecs-update-second", 80)],
        "case {case:?}: the accepted attachment must round-trip through DescribeServices"
    );

    let case = "an update onto a load balancer the account does not hold";
    let (code, message) = refusal(
        &router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": service_name,
            "loadBalancers": [classic_binding("ecs-update-absent", 80)],
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected an InvalidParameterException, got {code:?} with {message:?}"
    );
    assert_eq!(
        message,
        not_found("ecs-update-absent"),
        "case {case:?}: unexpected refusal message"
    );
    assert_eq!(
        stored_bindings(&router, service_name, case).await,
        vec![classic_binding("ecs-update-second", 80)],
        "case {case:?}: a refused update must leave the stored attachment alone"
    );

    // And an update that moves the service onto an `awsvpc` revision is judged against the
    // attachment the service ALREADY holds: the move changes how its tasks are reached, so
    // the pairing is checked even though the request names no `loadBalancers` list.
    let case = "an update onto an awsvpc revision, with a Classic attachment held";
    let (code, message) = refusal(
        &router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": service_name,
            "taskDefinition": "updateawsvpc",
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected an InvalidParameterException, got {code:?} with {message:?}"
    );
    assert_eq!(
        message,
        incompatible("ecs-update-second"),
        "case {case:?}: unexpected refusal message"
    );
    assert!(
        stored_task_definition(&router, service_name, case)
            .await
            .contains("updatebridge"),
        "case {case:?}: a refused update must leave the service on the revision it ran"
    );

    // A move onto another revision whose tasks a Classic load balancer CAN reach is
    // accepted, which is what makes the refusal above about the network mode and not about
    // naming a task definition at all.
    let case = "an update onto another bridge revision";
    ok(
        &router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": service_name,
            "taskDefinition": "updatebridge:2",
        }),
        case,
    )
    .await;
    let reported = stored_task_definition(&router, service_name, case).await;
    assert!(
        reported.ends_with("updatebridge:2"),
        "case {case:?}: expected the service to run updatebridge:2, got {reported:?}"
    );
    assert_eq!(
        stored_bindings(&router, service_name, case).await,
        vec![classic_binding("ecs-update-second", 80)],
        "case {case:?}: the accepted move must keep the attachment the service held"
    );
}
