// SPDX-License-Identifier: CC0-1.0

//! WHICH target group of a weighted `forward` action answers a request, end to end over the
//! wire, and the pin target-group stickiness hands the client.
//!
//! The rule under test: **a `forward` action distributes the requests it is matched for
//! across the target groups it names IN PROPORTION to each group's `Weight` — so a group at
//! weight 0 is never selected, the only group an action names answers every request, and two
//! groups at positive weights each answer a share of them — and where the action's
//! `TargetGroupStickinessConfig` is enabled, the answer carries a cookie naming the group
//! that answered and every later request carrying that cookie is answered by that same
//! group, for as long as the configuration states.**
//!
//! Every case drives the real wire path: an HTTP request to the name the load balancer
//! published, carrying no AWS credential, exactly as a client of that name sends it. WHICH
//! group answered is read off the answer the target itself produced — one Lambda function is
//! registered in both groups and returns the target group it was invoked FOR, which only this
//! service's own forward can have put in the event — so a row that reads a group's answer is
//! a row that reached that group.
//!
//! The rows differ along what the rule generalises over: the number of groups an action names
//! (one, two), the weights it names them at (unstated, zero beside positive in either
//! position, equal, unequal, every weight zero) and whether the action pins the client
//! (stickiness off, on with a duration, a pin the client replays, a pin no group answers to,
//! and a pin on a group a later weight of 0 excludes).

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState;
use smithy_server_runtime::MultiServiceRouter;

use crate::alb_data_plane::{create_load_balancer, send, settle, Answered};
use crate::provisioning_loop::{action, call, p, REGION};

/// The function both target groups register, and the module and attribute its package
/// carries the handler as.
const FUNCTION: &str = "weighted-forward-target";
const HANDLER: &str = "lambda_function.lambda_handler";

/// The runtime the package is created for.
const RUNTIME: &str = "python3.12";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern. No IAM is
/// mounted here, so nothing is known about it and nothing is refused for it.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The reserved cookie the load balancer pins a client to one target group with, from
/// `crate::state`'s reserved cookie prefixes ("Names that start with the following prefixes
/// are not allowed: AWSALB, AWSALBAPP, and AWSALBTG; they're reserved for use by the load
/// balancer").
const PIN_COOKIE: &str = "AWSALBTG";

/// The handler every request runs: it answers with the ARN of the target group the load
/// balancer forwarded the request TO, which the event carries as
/// `requestContext.elb.targetGroupArn`. One package serves both groups, so the body a row
/// reads names the group the forward chose and nothing else.
const HANDLER_SOURCE: &str = r#"
def lambda_handler(event, context):
    elb = (event.get("requestContext") or {}).get("elb") or {}
    return {
        "statusCode": 200,
        "statusDescription": "200 OK",
        "headers": {"content-type": "text/plain"},
        "body": elb.get("targetGroupArn") or "",
        "isBase64Encoded": False,
    }
"#;

/// A .zip deployment package holding [`HANDLER_SOURCE`] as `lambda_function.py`, written in
/// memory the way a caller uploads one.
fn package() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(HANDLER_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// This service and Lambda over ONE shared engine, as `core/router/src/lib.rs` mounts them:
/// the function the load balancer forwards to is RUN by that Lambda, so the answer a row
/// reads is one a real invocation produced.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// One Lambda `restJson1` call, as a client makes it.
async fn lambda(router: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://lambda.{REGION}.amazonaws.com{path}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The weights and the stickiness one row configures on the listener's default action.
struct Configured {
    /// One entry per target group the action names: its index in the groups below, and the
    /// `Weight` the request states for it — `None` leaving the member out.
    groups: &'static [(usize, Option<i32>)],
    /// The `TargetGroupStickinessConfig` the request states, if any.
    stickiness: Option<(bool, Option<i32>)>,
}

/// Write `configured` onto the listener through `ModifyListener`, the way a client shifts
/// weights.
async fn modify(
    router: &MultiServiceRouter,
    listener_arn: &str,
    groups: &[String],
    configured: &Configured,
) {
    let mut form = action("ModifyListener");
    form.push(p("ListenerArn", listener_arn));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    for (member, (index, weight)) in configured.groups.iter().enumerate() {
        let at = member + 1;
        form.push(p(
            &format!("DefaultActions.member.1.ForwardConfig.TargetGroups.member.{at}.TargetGroupArn"),
            &groups[*index],
        ));
        if let Some(weight) = weight {
            form.push(p(
                &format!("DefaultActions.member.1.ForwardConfig.TargetGroups.member.{at}.Weight"),
                &weight.to_string(),
            ));
        }
    }
    if let Some((enabled, duration)) = configured.stickiness {
        form.push(p(
            "DefaultActions.member.1.ForwardConfig.TargetGroupStickinessConfig.Enabled",
            if enabled { "true" } else { "false" },
        ));
        if let Some(duration) = duration {
            form.push(p(
                "DefaultActions.member.1.ForwardConfig.TargetGroupStickinessConfig.DurationSeconds",
                &duration.to_string(),
            ));
        }
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup ModifyListener: expected 200, got {} body {}",
        reply.status, reply.body
    );
}

/// One row of the weight table.
struct Row {
    case: &'static str,
    configured: Configured,
    /// How many requests the row sends.
    requests: usize,
    /// The status every one of them must be answered with.
    status: u16,
    /// The groups, by index, that may answer any of them.
    may_answer: &'static [usize],
    /// The groups, by index, that must each answer at least one of them.
    must_answer: &'static [usize],
}

/// The rows: every weighting a forward action can carry over one and two groups.
///
/// The counts are what makes an answered group's ABSENCE evidence rather than chance: a group
/// holding half of the interval is missed by 18 independent draws about once in 130,000 runs,
/// while a group excluded by a weight of 0 is missed with certainty. The SHARE each positive
/// weight receives is pinned exactly, draw by draw, in
/// `src/service/target_group_selection.rs`; what the rows add is that the whole wire path
/// honours it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the only group an action names, at no stated weight",
            configured: Configured {
                groups: &[(0, None)],
                stickiness: None,
            },
            requests: 3,
            status: 200,
            may_answer: &[0],
            must_answer: &[0],
        },
        Row {
            case: "a group at weight zero beside one at weight one",
            configured: Configured {
                groups: &[(0, Some(0)), (1, Some(1))],
                stickiness: None,
            },
            requests: 6,
            status: 200,
            may_answer: &[1],
            must_answer: &[1],
        },
        Row {
            case: "the mirror: the zero weight named second",
            configured: Configured {
                groups: &[(0, Some(1)), (1, Some(0))],
                stickiness: None,
            },
            requests: 6,
            status: 200,
            may_answer: &[0],
            must_answer: &[0],
        },
        Row {
            case: "two groups at equal weights",
            configured: Configured {
                groups: &[(0, Some(1)), (1, Some(1))],
                stickiness: None,
            },
            requests: 18,
            status: 200,
            may_answer: &[0, 1],
            must_answer: &[0, 1],
        },
        Row {
            // `TargetGroupTuple$Weight` is "[t]he weight. The range is 0 to 999", so this is
            // the widest split a forward can be given: the heavy group takes all but a
            // thousandth of the requests, and the light one is still named.
            case: "the published weight ceiling beside a one",
            configured: Configured {
                groups: &[(0, Some(999)), (1, Some(1))],
                stickiness: None,
            },
            requests: 6,
            status: 200,
            may_answer: &[0, 1],
            must_answer: &[0],
        },
        Row {
            case: "every weight zero: no group the action may select",
            configured: Configured {
                groups: &[(0, Some(0)), (1, Some(0))],
                stickiness: None,
            },
            requests: 2,
            status: 503,
            may_answer: &[],
            must_answer: &[],
        },
    ]
}

/// The target group the answer names, as the function reported it, or `None` for an answer no
/// target produced.
fn answered_group(answered: &Answered, groups: &[String]) -> Option<usize> {
    groups.iter().position(|arn| arn == &answered.body)
}

/// The value of the pin the answer minted, if it minted one.
fn minted_pin(answered: &Answered) -> Option<String> {
    let field = answered.header("set-cookie")?;
    let value = field
        .split(';')
        .next()
        .unwrap_or(field)
        .trim()
        .strip_prefix(&format!("{PIN_COOKIE}="))?;
    Some(value.to_string())
}

#[tokio::test]
async fn a_forward_distributes_its_requests_by_weight_and_pins_a_client_when_it_is_sticky() {
    let (router, engine) = emulator();

    // The function both groups register, created through Lambda's own API and RUN by the
    // Lambda emulator mounted beside this service.
    let (status, created) = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 30,
            "Code": { "ZipFile": aws_smithy_types::base64::encode(package()) },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected 201, got {status} {created}"
    );
    let function_arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup CreateFunction: no FunctionArn in {created}"))
        .to_string();
    // The activation span Lambda owes the function, placed rather than slept for — the same
    // reading every suite that invokes a freshly created function uses.
    {
        let state = engine
            .state::<service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for record in store.functions.values_mut() {
            record.active_due = past;
        }
    }

    let (lb_arn, dns_name) = create_load_balancer(&router, "weighted-alb").await;

    // Two `lambda` target groups, each registering that one function, and each granting the
    // load balancer the invocation for ITSELF — the grant is narrowed by the target group.
    let mut groups: Vec<String> = Vec::new();
    for name in ["weighted-blue", "weighted-green"] {
        let mut form = action("CreateTargetGroup");
        form.push(p("Name", name));
        form.push(p("TargetType", "lambda"));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "setup CreateTargetGroup {name}: expected 200, got {} body {}",
            reply.status, reply.body
        );
        let tg_arn = reply.text("TargetGroupArn").unwrap_or_else(|| {
            panic!("setup CreateTargetGroup {name}: no TargetGroupArn in {}", reply.body)
        });

        let mut form = action("RegisterTargets");
        form.push(p("TargetGroupArn", &tg_arn));
        form.push(p("Targets.member.1.Id", &function_arn));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "setup RegisterTargets {name}: expected 200, got {} body {}",
            reply.status, reply.body
        );

        let (status, granted) = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{FUNCTION}/policy"),
            json!({
                "StatementId": format!("elbv2-invoke-{name}"),
                "Action": "lambda:InvokeFunction",
                "Principal": "elasticloadbalancing.amazonaws.com",
                "SourceArn": tg_arn,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "setup AddPermission {name}: expected 201, got {status} {granted}"
        );
        groups.push(tg_arn);
    }

    // The HTTP:80 listener that forwards to both groups, created with the weights the first
    // row then replaces.
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p(
        "DefaultActions.member.1.ForwardConfig.TargetGroups.member.1.TargetGroupArn",
        &groups[0],
    ));
    form.push(p(
        "DefaultActions.member.1.ForwardConfig.TargetGroups.member.2.TargetGroupArn",
        &groups[1],
    ));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup CreateListener: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let listener_arn = reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("setup CreateListener: no ListenerArn in {}", reply.body));

    // The move to `active` the clock owes the load balancer, placed rather than slept for.
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));

    for row in rows() {
        modify(&router, &listener_arn, &groups, &row.configured).await;
        let mut answered_by: Vec<Option<usize>> = Vec::new();
        for _ in 0..row.requests {
            let answered = send(&router, "GET", &dns_name, "/pick", &[]).await;
            assert_eq!(
                answered.status, row.status,
                "case {:?}: expected HTTP {}, got {} with body {:?}",
                row.case, row.status, answered.status, answered.body
            );
            answered_by.push(answered_group(&answered, &groups));
        }
        for group in row.must_answer {
            assert!(
                answered_by.contains(&Some(*group)),
                "case {:?}: group {group} must answer at least one of {} requests, got {:?}",
                row.case,
                row.requests,
                answered_by
            );
        }
        for answered in &answered_by {
            match answered {
                Some(group) => assert!(
                    row.may_answer.contains(group),
                    "case {:?}: group {group} may not answer at all, got {:?}",
                    row.case,
                    answered_by
                ),
                None => assert!(
                    row.may_answer.is_empty(),
                    "case {:?}: every answer must name a group that may answer, got {:?}",
                    row.case,
                    answered_by
                ),
            }
        }
    }

    // STICKINESS OFF: the answers carry no pin at all.
    modify(
        &router,
        &listener_arn,
        &groups,
        &Configured {
            groups: &[(0, Some(1)), (1, Some(1))],
            stickiness: Some((false, None)),
        },
    )
    .await;
    for _ in 0..3 {
        let answered = send(&router, "GET", &dns_name, "/pick", &[]).await;
        assert_eq!(
            minted_pin(&answered),
            None,
            "case stickiness off: no pin may be minted, got {:?}",
            answered.header("set-cookie")
        );
    }

    // STICKINESS ON: the answer mints a pin for the group that answered, for the duration the
    // configuration states, and every request replaying it is answered by that same group —
    // and carries no second pin, because the client already holds one.
    let duration = 3600;
    modify(
        &router,
        &listener_arn,
        &groups,
        &Configured {
            groups: &[(0, Some(1)), (1, Some(1))],
            stickiness: Some((true, Some(duration))),
        },
    )
    .await;
    let first = send(&router, "GET", &dns_name, "/pick", &[]).await;
    let pinned_to = answered_group(&first, &groups)
        .unwrap_or_else(|| panic!("case stickiness on: no group answered {:?}", first.body));
    let field = first
        .header("set-cookie")
        .unwrap_or_else(|| panic!("case stickiness on: expected a pin, got no set-cookie"))
        .to_string();
    for expected in [
        format!("{PIN_COOKIE}="),
        "Path=/".to_string(),
        format!("Max-Age={duration}"),
    ] {
        assert!(
            field.contains(&expected),
            "case stickiness on: expected {expected:?} in the pin, got {field:?}"
        );
    }
    let pin = minted_pin(&first).unwrap_or_else(|| panic!("case stickiness on: {field:?}"));
    let carried = format!("{PIN_COOKIE}={pin}");
    for attempt in 0..6 {
        let answered = send(
            &router,
            "GET",
            &dns_name,
            "/pick",
            &[("cookie", carried.as_str())],
        )
        .await;
        assert_eq!(
            answered_group(&answered, &groups),
            Some(pinned_to),
            "case the pin replayed (attempt {attempt}): expected group {pinned_to}, got {:?}",
            answered.body
        );
        assert_eq!(
            minted_pin(&answered),
            None,
            "case the pin replayed (attempt {attempt}): a client holding the pin is handed no \
             second one, got {:?}",
            answered.header("set-cookie")
        );
    }

    // A pin NO group of the distribution answers to is no pin: the request is drawn and
    // answered, and a pin of its own is minted for it.
    let stale = send(
        &router,
        "GET",
        &dns_name,
        "/pick",
        &[("cookie", &format!("{PIN_COOKIE}=not-a-group-of-this-action"))],
    )
    .await;
    assert!(
        answered_group(&stale, &groups).is_some() && minted_pin(&stale).is_some(),
        "case a pin naming no group: expected a drawn answer carrying a fresh pin, got {:?} \
         with {:?}",
        stale.body,
        stale.header("set-cookie")
    );

    // A pin on a group a later weighting EXCLUDES is not honoured: the weight decides first,
    // so a client is not pinned to a group the action no longer selects.
    let drained = pinned_to;
    let serving = 1 - pinned_to;
    let mut weighted: Vec<(usize, Option<i32>)> = vec![(0, Some(1)), (1, Some(1))];
    weighted[drained] = (drained, Some(0));
    let configured = Configured {
        groups: match weighted.as_slice() {
            [(0, Some(0)), (1, Some(1))] => &[(0, Some(0)), (1, Some(1))],
            _ => &[(0, Some(1)), (1, Some(0))],
        },
        stickiness: Some((true, Some(duration))),
    };
    modify(&router, &listener_arn, &groups, &configured).await;
    for attempt in 0..4 {
        let answered = send(
            &router,
            "GET",
            &dns_name,
            "/pick",
            &[("cookie", carried.as_str())],
        )
        .await;
        assert_eq!(
            answered_group(&answered, &groups),
            Some(serving),
            "case the pinned group drained (attempt {attempt}): expected group {serving}, got \
             {:?}",
            answered.body
        );
    }

    // The serving wrote nothing: both groups still hold the one registration each, and no
    // pin is kept anywhere in the store.
    let state = engine.state::<ElbState>();
    let store = state.lock();
    for arn in &groups {
        let group = store
            .target_groups
            .get(arn)
            .unwrap_or_else(|| panic!("the target group {arn:?} is still held"));
        assert_eq!(
            group.targets.len(),
            1,
            "case the store after the serving: {arn:?} keeps its one registration, got {:?}",
            group.targets
        );
    }
}
