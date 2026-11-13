// SPDX-License-Identifier: CC0-1.0

//! A `lambda`-protocol subscription whose endpoint names a QUALIFIER of a function — a
//! published version or an alias — invokes exactly that qualifier, and only where THAT
//! qualifier's own resource-based policy grants Amazon SNS the invocation.
//!
//! The rule under test: **a publish to a topic delivers the message to every confirmed
//! `lambda`-protocol subscription of that topic ("lambda – delivery of JSON-encoded message to
//! an Lambda function", `SubscribeInput$Protocol`, whose endpoint "is the ARN of an Lambda
//! function") by invoking the function the endpoint ARN addresses asynchronously, and makes
//! that invocation only where the resource-based policy of the ADDRESSED resource grants
//! `lambda:InvokeFunction` to the service principal `sns.amazonaws.com` for the publishing
//! topic.** A function's ARN may carry a qualifier
//! (`arn:aws:lambda:<region>:<account>:function:<name>[:<qualifier>]`), a grant is attached to
//! the ARN `AddPermission` named — its `Qualifier` "specif[ies] a version or alias" — and an
//! invocation runs the code that qualifier froze, so one function reached through three
//! endpoints is three independent deliveries.
//!
//! This is the axis its siblings do not read. They assert the same authorization rule over the
//! UNQUALIFIED function ARN — as a stack declares it
//! ([`crate::sns_lambda_subscription_permission_boundary`],
//! [`crate::sns_lambda_cdk_subscription_delivery`]) and as a client reconciles the grant
//! afterwards ([`crate::sns_lambda_invoke_permission_revocation`]) — so what no suite pins is
//! the ENDPOINT FORM: three subscriptions of one topic addressing the same function at
//! `$LATEST`, at version `1` and through an alias, with the grant moved from one qualifier to
//! the next between rounds.
//!
//! Each round's observation is made where a client makes it: on the sink queue the invoked
//! handler forwards to, stamped with the ARN the run reports it was invoked as
//! (`context.invoked_function_arn`) and with a label baked into the deployment package, so one
//! body says both WHICH qualifier was invoked and WHOSE code ran. The alias and the published
//! version carry the package the stack created; `$LATEST` carries the one a stack update
//! replaced it with, which is how a delivery to the unqualified ARN is told from a delivery to
//! the version the alias resolves to.
//!
//! Three crates have to agree for that to hold (SNS fans the publish out, Lambda holds each
//! qualifier's policy and runs each qualifier's code, Amazon SQS holds what the handlers
//! wrote), and the stack that deploys the function is CloudFormation's, so the suite lives
//! here. Its router, its execution role and its awsQuery, Amazon SQS and Lambda callers are the
//! boundary suite's, so there is ONE harness for the readings of this rule rather than a copy
//! per reading.

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, elements, outputs_of};
use crate::sns_lambda_subscription_permission_boundary::{
    build_router, cloudformation, drain, execution_role, lambda, sns, sqs, ACCOUNT,
    CREATE_COMPLETE, LOOK_INTERVAL, ROLE_NAME,
};

/// The stack status an update reaches.
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The stack that deploys the function, the function it names, and the queue its handlers
/// forward to.
const STACK: &str = "sns-lambda-qualified";
const FUNCTION: &str = "sns-lambda-qualified-forwarder";
const SINK_QUEUE: &str = "sns-lambda-qualified-sink";

/// The alias the published version is reached through.
const ALIAS: &str = "live";

/// The published version the alias resolves to: the first version of the function, which is
/// the one `PublishVersion` mints for a function that has none.
const VERSION: &str = "1";

/// The label the package the stack CREATES stamps on what it forwards, and the one the package
/// a stack UPDATE replaces it with stamps. A published version keeps the package it was
/// published from, so the first label is what the version and the alias run and the second is
/// what `$LATEST` runs.
const PUBLISHED_CODE: &str = "published-code";
const UPDATED_CODE: &str = "updated-code";

/// The service principal a delivery to a subscribed function is performed as, and the action it
/// performs on that function.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The statement id every grant below is added and removed under: a client reconciling a grant
/// addresses the statement by id, which is what `RemovePermission` takes.
const STATEMENT_ID: &str = "sns-invoke";

/// What separates the members of one forwarded body: the ARN the run was invoked as, the label
/// of the package that ran, and the message that was published. None of the three can hold it.
const SEPARATOR: char = '|';

/// How long a round that must observe a delivery keeps looking for it. Generous: the
/// invocation is asynchronous, the handler is a real interpreted package, and a round that
/// admits three deliveries runs three of them.
const ARRIVAL_BUDGET: Duration = Duration::from_secs(90);

/// How long a round that must observe NOTHING keeps LOOKING for one before concluding nothing
/// came, and how long the final reading keeps re-checking the whole run. It is the span a
/// delivery admitted in the same fan-out arrives inside of, with room to spare; a delivery
/// slower than that is caught by the final reading, which re-checks every round's identities
/// over this span again.
const QUIET_BUDGET: Duration = Duration::from_secs(10);

/// How long a round that HAS observed what it expects keeps looking past it, so a delivery to a
/// qualifier the round did not grant is caught by that round rather than only at the end.
const GRACE: Duration = Duration::from_secs(2);

/// The handler the stack deploys: it forwards every SNS record it is handed to the sink queue,
/// stamped with the ARN this run was invoked as and the label of the package that is running.
///
/// The queue is addressed by NAME, resolved through the service's own `GetQueueUrl`, so the
/// source carries nothing the deployment had to compute. The label is baked into the SOURCE
/// rather than passed in the environment, because what the two deployments have to differ in
/// is the deployment PACKAGE a version freezes.
const FORWARDING_SOURCE: &str = r#"
import boto3


CODE = "CODE_LABEL"


def lambda_handler(event, context):
    client = boto3.client("sqs")
    url = client.get_queue_url(QueueName="SINK_NAME")["QueueUrl"]
    forwarded = 0
    for record in event["Records"]:
        client.send_message(
            QueueUrl=url,
            MessageBody=SEPARATOR.join(
                [context.invoked_function_arn, CODE, record["Sns"]["Message"]]
            ),
        )
        forwarded += 1
    return {"forwarded": forwarded}
"#;

/// The template the stack is deployed with: the one function, carrying the package whose label
/// is `code`.
fn template(code: &str) -> String {
    let source = FORWARDING_SOURCE
        .replace("CODE_LABEL", code)
        .replace("SINK_NAME", SINK_QUEUE)
        .replace("SEPARATOR", &format!("{SEPARATOR:?}"));
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Forwarder": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Role": format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}"),
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Timeout": 10,
                    "Code": { "ZipFile": source },
                },
            },
        },
        "Outputs": {
            "FunctionArn": { "Value": { "Fn::GetAtt": ["Forwarder", "Arn"] } },
        },
    })
    .to_string()
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function ready for an
/// invocation — `Active`, with no update still being applied — and assert it got there. The
/// span a function spends coming up is that service's own, so nothing here sleeps a fixed span
/// on its behalf.
async fn wait_until_ready(router: &MultiServiceRouter, case: &str) {
    let deadline = Instant::now() + ARRIVAL_BUDGET;
    loop {
        let (_, reported) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await;
        let state = reported["State"].as_str().unwrap_or_default().to_string();
        let update = reported["LastUpdateStatus"]
            .as_str()
            .unwrap_or("Successful")
            .to_string();
        if (state == "Active" && update == "Successful") || Instant::now() >= deadline {
            assert_eq!(
                (state.as_str(), update.as_str()),
                ("Active", "Successful"),
                "case {case:?}: expected Lambda to report {FUNCTION} ready to be invoked, got \
                 {reported}"
            );
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The source a grant's `aws:SourceArn` condition names, which is what scopes a grant to the
/// topic whose notifications it admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// No condition at all: the grant admits this service whatever it publishes from.
    Unscoped,
    /// The topic the round publishes to.
    ThisTopic,
    /// Another topic of the same account, which this round does not publish to.
    OtherTopic,
}

/// One grant a round puts in force: the qualifier of the function it is attached to (by the
/// index of the endpoint addressing it), and what scopes it.
#[derive(Debug, Clone, Copy)]
struct Grant {
    endpoint: usize,
    source: Source,
}

/// One round: the grants in force when it publishes, and which endpoints must therefore be
/// invoked (by index).
struct Round {
    case: &'static str,
    grants: &'static [Grant],
    invoked: &'static [usize],
}

/// The endpoints, by index: the unqualified function ARN, the published version's ARN and the
/// alias's ARN.
const UNQUALIFIED: usize = 0;
const PUBLISHED: usize = 1;
const ALIASED: usize = 2;

/// The rounds, in order, over one topic and the three subscriptions of it. They differ along
/// what the rule generalises over: WHICH qualifier of the one function holds the grant, whether
/// the statement is scoped to the publishing topic, to another one or to nothing, and how many
/// qualifiers hold one at once.
const ROUNDS: &[Round] = &[
    Round {
        case: "no qualifier of the subscribed function grants this service the invocation",
        grants: &[],
        invoked: &[],
    },
    Round {
        case: "the alias alone grants it, scoped to the publishing topic",
        grants: &[Grant {
            endpoint: ALIASED,
            source: Source::ThisTopic,
        }],
        invoked: &[ALIASED],
    },
    Round {
        case: "the published version alone grants it, unscoped",
        grants: &[Grant {
            endpoint: PUBLISHED,
            source: Source::Unscoped,
        }],
        invoked: &[PUBLISHED],
    },
    Round {
        case: "the unpublished version alone grants it, scoped to the publishing topic",
        grants: &[Grant {
            endpoint: UNQUALIFIED,
            source: Source::ThisTopic,
        }],
        invoked: &[UNQUALIFIED],
    },
    Round {
        case: "the alias grants it for another topic only",
        grants: &[Grant {
            endpoint: ALIASED,
            source: Source::OtherTopic,
        }],
        invoked: &[],
    },
    Round {
        case: "every qualifier grants it",
        grants: &[
            Grant {
                endpoint: UNQUALIFIED,
                source: Source::ThisTopic,
            },
            Grant {
                endpoint: PUBLISHED,
                source: Source::ThisTopic,
            },
            Grant {
                endpoint: ALIASED,
                source: Source::Unscoped,
            },
        ],
        invoked: &[UNQUALIFIED, PUBLISHED, ALIASED],
    },
];

/// One endpoint of the fan-out: the ARN a subscription addresses, the qualifier a grant on it
/// names (and `None` for the unpublished version, whose grants carry no qualifier — "Lambda
/// does not support adding policies to version $LATEST"), and the label of the package that
/// qualifier runs.
struct Endpoint {
    case: &'static str,
    arn: String,
    qualifier: Option<&'static str>,
    code: &'static str,
}

/// The identity one forwarded body reports: the ARN the run was invoked as and the label of the
/// package that ran.
fn identity(arn: &str, code: &str) -> String {
    format!("{arn}{SEPARATOR}{code}")
}

/// The identities among `seen` whose message is `marker`, sorted so a round's reading is
/// compared as a multiset rather than in arrival order.
fn carrying(seen: &[String], marker: &str) -> Vec<String> {
    let mut held: Vec<String> = seen
        .iter()
        .filter_map(|body| {
            let mut members = body.splitn(3, SEPARATOR);
            let arn = members.next()?;
            let code = members.next()?;
            let message = members.next()?;
            (message == marker).then(|| identity(arn, code))
        })
        .collect();
    held.sort();
    held
}

/// Every identity the sink has reported for `marker`, read by LOOKING AGAIN until the round's
/// expectation holds — `wanted` of them — and then over [`GRACE`] past that, so a delivery the
/// round did not admit is caught by this round too. A round expecting none keeps looking for
/// [`QUIET_BUDGET`]. Nothing here waits a fixed span in place of looking: every wait is one
/// turn of a loop whose condition is the reading itself.
///
/// `seen` accumulates every body the queue ever handed out, across rounds: a delivery that
/// arrives after the round that published it was asserted is still in there for the final
/// reading to catch.
async fn forwards_of(
    router: &MultiServiceRouter,
    sink: &str,
    seen: &mut Vec<String>,
    marker: &str,
    wanted: usize,
) -> Vec<String> {
    let deadline = Instant::now()
        + if wanted == 0 {
            QUIET_BUDGET
        } else {
            ARRIVAL_BUDGET
        };
    // When the expectation is first met the watch runs on to this instant rather than stopping:
    // that is what makes a SECOND delivery of the same publish visible to this round.
    let mut past_arrival: Option<Instant> = None;
    loop {
        seen.extend(drain(router, sink).await);
        let held = carrying(seen, marker);
        let now = Instant::now();
        if past_arrival.is_none() && wanted > 0 && held.len() >= wanted {
            past_arrival = Some(now + GRACE);
        }
        if now >= deadline || past_arrival.is_some_and(|until| now >= until) {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Grant this service the invocation of the qualifier `endpoint` addresses, scoped as `source`
/// says.
async fn add_permission(
    router: &MultiServiceRouter,
    case: &str,
    endpoint: &Endpoint,
    source: Source,
    topic: &str,
    other_topic: &str,
) {
    let mut body = json!({
        "StatementId": STATEMENT_ID,
        "Action": INVOKE_ACTION,
        "Principal": SNS_PRINCIPAL,
    });
    match source {
        Source::Unscoped => {}
        Source::ThisTopic => body["SourceArn"] = json!(topic),
        Source::OtherTopic => body["SourceArn"] = json!(other_topic),
    }
    let (status, answered) = lambda(router, "POST", &policy_uri(endpoint), Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from AddPermission on {:?}, got {status} with \
         {answered}",
        endpoint.case
    );
}

/// Take that grant away again, which is the call a client reconciling a fan-out's permissions
/// makes.
async fn remove_permission(router: &MultiServiceRouter, case: &str, endpoint: &Endpoint) {
    let uri = match endpoint.qualifier {
        Some(qualifier) => format!(
            "/2015-03-31/functions/{FUNCTION}/policy/{STATEMENT_ID}?Qualifier={qualifier}"
        ),
        None => format!("/2015-03-31/functions/{FUNCTION}/policy/{STATEMENT_ID}"),
    };
    let (status, answered) = lambda(router, "DELETE", &uri, None).await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected HTTP 204 from RemovePermission on {:?}, got {status} with \
         {answered}",
        endpoint.case
    );
}

/// Where a grant on the qualifier `endpoint` addresses is added: the function's policy, with
/// the `Qualifier` that names the version or alias the statement is attached to.
fn policy_uri(endpoint: &Endpoint) -> String {
    match endpoint.qualifier {
        Some(qualifier) => {
            format!("/2015-03-31/functions/{FUNCTION}/policy?Qualifier={qualifier}")
        }
        None => format!("/2015-03-31/functions/{FUNCTION}/policy"),
    }
}

/// Every round, over one topic and the three subscriptions of one function: each publish
/// invokes exactly the qualifiers whose own policy admits this service at that moment, and the
/// code that runs is the one that qualifier froze.
#[tokio::test]
async fn a_publish_invokes_exactly_the_subscribed_function_qualifiers_that_admit_it() {
    let router = build_router();
    let setup = "setup";

    // The execution role the function is created with, and the sink its handlers forward to.
    // The sink's visibility window is past the whole suite, so a body one round read cannot
    // come back visible and be counted again in a later one.
    execution_role(&router).await;
    let created_sink = sqs(
        &router,
        "CreateQueue",
        &json!({
            "QueueName": SINK_QUEUE,
            "Attributes": { "VisibilityTimeout": "900" },
        }),
    )
    .await;
    let sink = created_sink["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {setup:?}: CreateQueue answered {created_sink}"))
        .to_string();

    // The function, deployed with the package the published version will freeze.
    cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template(PUBLISHED_CODE)),
        ],
    )
    .await;
    let (status, described) = wait_for_stack_status(&router, STACK, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "case {setup:?}: expected the stack to reach {CREATE_COMPLETE}, got {described}"
    );
    let function_arn = outputs_of(&described)
        .get("FunctionArn")
        .cloned()
        .unwrap_or_else(|| panic!("case {setup:?}: the stack published no FunctionArn"));
    wait_until_ready(&router, setup).await;

    // The version that freezes that package, and the alias that resolves to it.
    let (status, published) = lambda(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/versions"),
        None,
    )
    .await;
    assert_eq!(
        status, 201,
        "case {setup:?}: expected HTTP 201 from PublishVersion, got {status} with {published}"
    );
    assert_eq!(
        published["Version"].as_str(),
        Some(VERSION),
        "case {setup:?}: expected the first published version to be {VERSION:?}, got {published}"
    );
    let (status, alias) = lambda(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/aliases"),
        Some(json!({ "Name": ALIAS, "FunctionVersion": VERSION })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {setup:?}: expected HTTP 201 from CreateAlias, got {status} with {alias}"
    );
    assert_eq!(
        alias["AliasArn"].as_str(),
        Some(format!("{function_arn}:{ALIAS}").as_str()),
        "case {setup:?}: expected the alias to be named under the function's ARN, got {alias}"
    );

    // The update that replaces the package of the UNPUBLISHED version only: from here the
    // three endpoints run two different packages.
    cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template(UPDATED_CODE)),
        ],
    )
    .await;
    let (status, described) = wait_for_stack_status(&router, STACK, UPDATE_COMPLETE).await;
    let reasons = if status == UPDATE_COMPLETE {
        Vec::new()
    } else {
        let events = cloudformation(&router, "DescribeStackEvents", &[("StackName", STACK)]).await;
        elements(&events, "ResourceStatusReason")
    };
    assert_eq!(
        status, UPDATE_COMPLETE,
        "case {setup:?}: expected the update to reach {UPDATE_COMPLETE}, got {described} with \
         resource failures {reasons:?}"
    );
    wait_until_ready(&router, setup).await;

    let endpoints = [
        Endpoint {
            case: "the unqualified function ARN",
            arn: function_arn.clone(),
            qualifier: None,
            code: UPDATED_CODE,
        },
        Endpoint {
            case: "the published version's ARN",
            arn: format!("{function_arn}:{VERSION}"),
            qualifier: Some(VERSION),
            code: PUBLISHED_CODE,
        },
        Endpoint {
            case: "the alias's ARN",
            arn: format!("{function_arn}:{ALIAS}"),
            qualifier: Some(ALIAS),
            code: PUBLISHED_CODE,
        },
    ];

    // The topic every round publishes to, one a grant may be scoped to instead, and the three
    // subscriptions — one per endpoint form.
    let topic = element(&sns(&router, "CreateTopic", &[("Name", STACK)]).await, "TopicArn");
    let other_topic = element(
        &sns(&router, "CreateTopic", &[("Name", "sns-lambda-qualified-other")]).await,
        "TopicArn",
    );
    assert!(
        !topic.is_empty() && !other_topic.is_empty(),
        "case {setup:?}: both topics must be created, got {topic:?} and {other_topic:?}"
    );
    for endpoint in &endpoints {
        let subscribed = sns(
            &router,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "lambda"),
                ("Endpoint", &endpoint.arn),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
        let arn = element(&subscribed, "SubscriptionArn");
        assert!(
            arn.starts_with(&topic),
            "case {:?}: subscribing {} must answer a confirmed subscription of the topic, got \
             {subscribed}",
            endpoint.case,
            endpoint.arn
        );
    }

    let mut seen: Vec<String> = Vec::new();
    let mut held: Vec<usize> = Vec::new();
    let mut expected_overall: Vec<String> = Vec::new();

    for (index, round) in ROUNDS.iter().enumerate() {
        let case = round.case;

        // Reconcile the grants to the ones this round publishes under: a statement is
        // addressed by its id, so a grant whose scope changed is removed and added again.
        for endpoint in held.drain(..) {
            remove_permission(&router, case, &endpoints[endpoint]).await;
        }
        for grant in round.grants {
            let endpoint = &endpoints[grant.endpoint];
            add_permission(&router, case, endpoint, grant.source, &topic, &other_topic).await;
            held.push(grant.endpoint);
        }

        let marker = format!("marker-{index}");
        let published = sns(
            &router,
            "Publish",
            &[("TopicArn", &topic), ("Message", &marker)],
        )
        .await;
        assert!(
            !element(&published, "MessageId").is_empty(),
            "case {case:?}: Publish must answer a MessageId whether or not a delivery is \
             admitted, got {published}"
        );

        let mut wanted: Vec<String> = round
            .invoked
            .iter()
            .map(|index| identity(&endpoints[*index].arn, endpoints[*index].code))
            .collect();
        wanted.sort();
        expected_overall.extend(wanted.iter().cloned());

        let forwarded = forwards_of(&router, &sink, &mut seen, &marker, wanted.len()).await;
        assert_eq!(
            forwarded, wanted,
            "case {case:?}: expected the subscribed qualifiers {wanted:?} to be invoked for \
             {marker:?} and no others, got {forwarded:?} (every body seen so far: {seen:?})"
        );
    }

    // One last watch over the whole run: the reading has to STAY the one every round admitted,
    // so the sink is read again in a loop and checked each time — a delivery that arrives after
    // the round that would have caught it fails here the moment it lands, rather than after a
    // fixed wait.
    expected_overall.sort();
    let deadline = Instant::now() + QUIET_BUDGET;
    loop {
        seen.extend(drain(&router, &sink).await);
        let mut overall = identities(&seen);
        overall.sort();
        assert_eq!(
            overall, expected_overall,
            "expected exactly one invocation per admitted delivery across every round, got \
             {overall:?}"
        );
        if Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The identity every forwarded body among `seen` reports, whatever it was published for: what
/// the whole run comes to, read the same way one round's reading is ([`carrying`]).
fn identities(seen: &[String]) -> Vec<String> {
    seen.iter()
        .filter_map(|body| {
            let mut members = body.splitn(3, SEPARATOR);
            let arn = members.next()?;
            let code = members.next()?;
            members.next()?;
            Some(identity(arn, code))
        })
        .collect()
}
