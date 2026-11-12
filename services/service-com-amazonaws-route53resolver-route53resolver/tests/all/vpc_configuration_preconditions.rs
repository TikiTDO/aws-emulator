// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resource preconditions of the per-VPC configuration
//! operations and of the query-logging create.
//!
//! What they pin is the rule all of them follow: the resource a request names
//! has to be one the ACCOUNT holds — a VPC of its own Amazon EC2 inventory for
//! the per-VPC configurations, and a log destination of the service that owns it
//! for a query logging configuration — so a request naming a resource the
//! account does not hold is refused with that operation's modelled client error
//! and nothing is reported or written for it.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`]; the stores are read directly only to count the
//! records a refused call left behind.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    canonical_query_log_destination, service_state, RSLVR_CODE_ACCESS_DENIED,
    RSLVR_CODE_RESOLVER_RESOURCE_NOT_FOUND, RSLVR_CODE_RESOURCE_NOT_FOUND,
};

use super::harness::{call, emulator, emulator_on, error_type, ok, string_member, Emulator};

/// What the `Message` of a refusal states.
enum Refusal {
    /// The coded can't-find sentence naming the id from the request, closed with
    /// the trace clause of the request.
    TracedNotFound(&'static str),
    /// The same coded sentence, and nothing else.
    NotFound(&'static str),
    /// The coded denial, which states the fault and NOTHING about the resource
    /// the request named.
    Denied(&'static str),
}

/// One VPC-scoped configuration operation: how a request names the VPC it is
/// about, and what the service answers for a VPC the account does not hold.
struct VpcConfiguration {
    operation: &'static str,
    /// The members a request carries beside `ResourceId`.
    extra: fn() -> Value,
    /// The member the answer carries the configuration in.
    member: &'static str,
    /// The modelled error shape the refusal names.
    error: &'static str,
    /// What the refusal's message states.
    refusal: Refusal,
}

/// Every operation the rule holds for: the three per-VPC singleton
/// configurations this service maintains, each read and written.
///
/// The error each one answers is the one ITS model declares for a resource the
/// caller cannot address
/// (`smithy_query(query="operation",operation="GetFirewallConfig,...")`): the
/// firewall and Resolver behavior configurations report
/// `ResourceNotFoundException` — under the DNS Firewall code and the Resolver
/// code respectively, and only the DNS Firewall slice closes its refusal with
/// the trace clause, as the recorded responses of the two carry them — and the
/// DNSSEC validation configuration reports `AccessDeniedException`, which
/// discloses nothing about whether the VPC exists.
const CONFIGURATIONS: &[VpcConfiguration] = &[
    VpcConfiguration {
        operation: "GetFirewallConfig",
        extra: || json!({}),
        member: "FirewallConfig",
        error: "ResourceNotFoundException",
        refusal: Refusal::TracedNotFound(RSLVR_CODE_RESOURCE_NOT_FOUND),
    },
    VpcConfiguration {
        operation: "UpdateFirewallConfig",
        extra: || json!({ "FirewallFailOpen": "ENABLED" }),
        member: "FirewallConfig",
        error: "ResourceNotFoundException",
        refusal: Refusal::TracedNotFound(RSLVR_CODE_RESOURCE_NOT_FOUND),
    },
    VpcConfiguration {
        operation: "GetResolverConfig",
        extra: || json!({}),
        member: "ResolverConfig",
        error: "ResourceNotFoundException",
        refusal: Refusal::NotFound(RSLVR_CODE_RESOLVER_RESOURCE_NOT_FOUND),
    },
    VpcConfiguration {
        operation: "UpdateResolverConfig",
        extra: || json!({ "AutodefinedReverseFlag": "DISABLE" }),
        member: "ResolverConfig",
        error: "ResourceNotFoundException",
        refusal: Refusal::NotFound(RSLVR_CODE_RESOLVER_RESOURCE_NOT_FOUND),
    },
    VpcConfiguration {
        operation: "GetResolverDnssecConfig",
        extra: || json!({}),
        member: "ResolverDNSSECConfig",
        error: "AccessDeniedException",
        refusal: Refusal::Denied(RSLVR_CODE_ACCESS_DENIED),
    },
    VpcConfiguration {
        operation: "UpdateResolverDnssecConfig",
        extra: || json!({ "Validation": "ENABLE" }),
        member: "ResolverDNSSECConfig",
        error: "AccessDeniedException",
        refusal: Refusal::Denied(RSLVR_CODE_ACCESS_DENIED),
    },
];

/// Whether `clause` is a trace id in double quotes, in the form AWS X-Ray gives
/// one: `1-<8 hex>-<24 hex>` (X-Ray's own `TraceId`, read through the one
/// reading of that form the services share).
fn is_quoted_trace_id(clause: &str) -> bool {
    clause
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .is_some_and(service_common::xray_trace::is_trace_id)
}

/// The request `configuration` makes about `resource_id`.
fn request(configuration: &VpcConfiguration, resource_id: &str) -> Value {
    let mut body = (configuration.extra)();
    body.as_object_mut()
        .expect("the request body is an object")
        .insert("ResourceId".to_string(), json!(resource_id));
    body
}

/// How many per-VPC configuration records this service's store holds, across all
/// three slices.
fn stored_configurations(engine: &engine::InMemoryEngine) -> usize {
    let state = service_state(engine)
        .expect("the store is reachable from the engine the service is mounted on");
    let store = state.lock();
    store.firewall_configs.len() + store.resolver_configs.len() + store.resolver_dnssec_configs.len()
}

/// Every per-VPC configuration operation refuses a `ResourceId` that names no
/// VPC of the account — with the error ITS model declares for a resource the
/// caller cannot address, naming the id from the request where the service
/// names it at all — and writes nothing; the very same call is served for a VPC
/// the account does hold.
///
/// The rows span what the rule generalises over, the form of the VPC id
/// (`ResourceId`, length 1..64, is the only constraint the model puts on it):
/// the two id widths Amazon EC2 mints, both ends of the modelled length, and an
/// id in no form AWS mints. Each row is asserted against all six operations.
#[tokio::test]
async fn a_vpc_the_account_does_not_hold_is_refused_by_every_configuration_operation() {
    let unheld: Vec<(&str, String)> = vec![
        ("the 8-hex-digit form", "vpc-5a4b3c2d".to_string()),
        ("the 17-hex-digit form", "vpc-0123456789abcdef0".to_string()),
        ("one character, the modelled minimum", "u".to_string()),
        (
            "64 characters, the modelled maximum",
            format!("vpc-{}", "1".repeat(60)),
        ),
        ("an id in no form AWS mints", "lab vpc #9".to_string()),
    ];

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    for (row, resource_id) in &unheld {
        for configuration in CONFIGURATIONS {
            let case = format!("{} / {row}", configuration.operation);
            let (status, body) = call(
                &router,
                configuration.operation,
                request(configuration, resource_id),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a VPC the account does not hold must be refused, got {status}: {body}"
            );
            assert!(
                error_type(&body).contains(configuration.error),
                "case {case:?}: the refusal must name {}, got {body}",
                configuration.error
            );
            assert!(
                body.get(configuration.member).is_none(),
                "case {case:?}: a refusal carries no configuration, got {body}"
            );
            let message = string_member(&body, "Message");
            let not_found =
                |code: &str| format!("[{code}] Can’t find the resource with ID \"{resource_id}\".");
            match configuration.refusal {
                // The DNS Firewall refusal states the coded sentence and closes
                // with the trace clause of the request, whose id is this call's
                // own and never anything the caller asked about.
                Refusal::TracedNotFound(code) => {
                    let (stated, trace) = message.split_once(" Trace Id: ").unwrap_or_else(|| {
                        panic!("case {case:?}: the refusal must close with its trace clause, got {body}")
                    });
                    assert_eq!(
                        stated,
                        not_found(code),
                        "case {case:?}: the refusal must state the coded sentence naming the id, got {body}"
                    );
                    assert!(
                        is_quoted_trace_id(trace),
                        "case {case:?}: the trace clause must carry this request's trace id in quotes, got {body}"
                    );
                }
                Refusal::NotFound(code) => assert_eq!(
                    message,
                    not_found(code),
                    "case {case:?}: the refusal must state the coded sentence naming the id, got {body}"
                ),
                // The denial states the fault and NOTHING from the request, so
                // every row of it answers the one sentence, closed with the
                // trace clause of the call it refused.
                Refusal::Denied(code) => {
                    let (stated, trace) = message.split_once(" Trace Id: ").unwrap_or_else(|| {
                        panic!("case {case:?}: the denial must close with its trace clause, got {body}")
                    });
                    assert_eq!(
                        stated,
                        format!("[{code}] Access to resource is denied."),
                        "case {case:?}: the denial must state the coded fault and nothing of the request, got {body}"
                    );
                    assert!(
                        is_quoted_trace_id(trace),
                        "case {case:?}: the trace clause must carry this request's trace id in quotes, got {body}"
                    );
                }
            }
        }
    }

    // Not one of the refused calls wrote a record, so a refused write leaves the
    // store exactly as it was.
    let held = stored_configurations(&engine);
    assert_eq!(
        held, 0,
        "a refused call must store no configuration, got {held} stored"
    );

    // The same calls are served for a VPC the account holds, so the refusals
    // above are about the VPC and not about refusing the operation.
    let vpc_id = "vpc-0abcdef0123456789";
    router.hold_vpcs([vpc_id]);
    for configuration in CONFIGURATIONS {
        let case = format!("{} / a VPC the account holds", configuration.operation);
        let served = ok(
            &router,
            configuration.operation,
            request(configuration, vpc_id),
        )
        .await;
        let reported = served.get(configuration.member).unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} must answer a {}, got {served}",
                configuration.operation, configuration.member
            )
        });
        assert_eq!(
            string_member(reported, "ResourceId"),
            vpc_id,
            "case {case:?}: the configuration must be the one the VPC was asked about, got {reported}"
        );
    }

    // A VPC the account released again is refused once more, so the precondition
    // follows the inventory rather than anything this service remembers.
    router.release_vpc(vpc_id);
    for configuration in CONFIGURATIONS {
        let case = format!("{} / a VPC the account released", configuration.operation);
        let (status, body) = call(
            &router,
            configuration.operation,
            request(configuration, vpc_id),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a VPC the account no longer holds must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains(configuration.error),
            "case {case:?}: the refusal must name {}, got {body}",
            configuration.error
        );
    }
}

/// `ResolverConfig$ResourceId` is "the ID of the Amazon Virtual Private Cloud
/// VPC or a Route 53 Profile that you're configuring Resolver for", so the
/// Resolver behavior configuration is READ and WRITTEN for a Route 53 Profile
/// the account holds, exactly as for one of its VPCs — and is refused for an id
/// the account holds neither under.
///
/// The rows span the forms the id takes: the `rp-<hex>` form that service mints
/// and one in no form AWS mints, because what decides the answer is the
/// inventory and not the shape of the id.
#[tokio::test]
async fn a_profile_the_account_holds_carries_a_resolver_behavior_configuration() {
    let router = emulator();
    let rows = [
        (
            "the rp-<hex> form Route 53 Profiles mints",
            "rp-0123456789abcdef",
        ),
        ("an id in no form AWS mints", "profile #4"),
    ];
    // The two operations of the slice whose `ResourceId` admits a Profile.
    let behavior: Vec<&VpcConfiguration> = CONFIGURATIONS
        .iter()
        .filter(|configuration| configuration.member == "ResolverConfig")
        .collect();

    for (row, profile_id) in rows {
        // An id the account holds no Profile and no VPC under is refused.
        for configuration in &behavior {
            let case = format!("{} / {row}, not held", configuration.operation);
            let (status, body) = call(
                &router,
                configuration.operation,
                request(configuration, profile_id),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: an id the account holds nothing under must be refused, got {status}: {body}"
            );
            assert!(
                error_type(&body).contains(configuration.error),
                "case {case:?}: the refusal must name {}, got {body}",
                configuration.error
            );
        }

        // Once the account holds that Profile, the very same calls are served
        // and report the configuration of the resource they named.
        router.hold_profile(profile_id);
        for configuration in &behavior {
            let case = format!("{} / {row}, held", configuration.operation);
            let served = ok(
                &router,
                configuration.operation,
                request(configuration, profile_id),
            )
            .await;
            let reported = served.get(configuration.member).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {} must answer a {}, got {served}",
                    configuration.operation, configuration.member
                )
            });
            assert_eq!(
                string_member(reported, "ResourceId"),
                profile_id,
                "case {case:?}: the configuration must be the one the Profile was asked about, got {reported}"
            );
        }
    }
}

/// One row of the destination table: a `DestinationArn` of one of the kinds
/// Resolver delivers query logs to.
struct Destination {
    case: &'static str,
    arn: &'static str,
}

/// One destination of every kind `CreateResolverQueryLogConfig$DestinationArn`
/// documents — "an S3 bucket, a CloudWatch Logs log group, or a Kinesis Data
/// Firehose delivery stream" — in each of the ARN forms the emulator accepts: a
/// bucket bare and with the file prefix the doc allows, a log group with and
/// without the `:*` log-stream wildcard, and a stream under both the `kinesis`
/// and the `firehose` service names.
const DESTINATIONS: &[Destination] = &[
    Destination {
        case: "an S3 bucket",
        arn: "arn:aws:s3:::amzn-s3-demo-bucket-precondition",
    },
    Destination {
        case: "an S3 bucket with a file prefix",
        arn: "arn:aws:s3:::amzn-s3-demo-bucket-prefixed/queries/",
    },
    Destination {
        case: "a log group named with the stream wildcard",
        arn: "arn:aws:logs:us-west-2:123456789012:log-group:/queries-wildcard:*",
    },
    Destination {
        case: "a log group named without the wildcard",
        arn: "arn:aws:logs:us-west-2:123456789012:log-group:/queries-bare",
    },
    Destination {
        case: "a Kinesis data stream",
        arn: "arn:aws:kinesis:us-east-2:012345678901:stream/queries-stream",
    },
    Destination {
        case: "a Firehose delivery stream",
        arn: "arn:aws:firehose:eu-west-1:012345678901:deliverystream/queries-delivery",
    },
];

/// The `Id`s `ListResolverQueryLogConfigs` reports.
async fn listed_ids(router: &Emulator) -> Vec<String> {
    let page = ok(router, "ListResolverQueryLogConfigs", json!({})).await;
    page.get("ResolverQueryLogConfigs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|config| string_member(config, "Id"))
        .collect()
}

/// A query logging configuration is created only for a destination the account
/// HOLDS: a well-formed `DestinationArn` whose bucket, log group or stream the
/// owning service does not hold is refused with the `AccessDeniedException` the
/// operation models and creates nothing, and the very same create is served once
/// the account holds that destination.
///
/// The rows span what the rule generalises over: every kind of destination the
/// member's doc names, in each ARN form this service accepts.
#[tokio::test]
async fn a_destination_the_account_does_not_hold_is_refused_and_creates_nothing() {
    let router = emulator();

    for (index, destination) in DESTINATIONS.iter().enumerate() {
        let case = destination.case;
        let body = json!({
            "CreatorRequestId": format!("req-precondition-{index}"),
            "Name": format!("precondition-{index}"),
            "DestinationArn": destination.arn,
        });

        let (status, refused) = call(&router, "CreateResolverQueryLogConfig", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a destination the account does not hold must be refused, got {status}: {refused}"
        );
        assert!(
            error_type(&refused).contains("AccessDeniedException"),
            "case {case:?}: the refusal must name AccessDeniedException, got {refused}"
        );
        assert!(
            refused.get("ResolverQueryLogConfig").is_none(),
            "case {case:?}: a refusal carries no configuration, got {refused}"
        );
        assert!(
            string_member(&refused, "Message")
                .starts_with(&format!("[{RSLVR_CODE_ACCESS_DENIED}] ")),
            "case {case:?}: the refusal must state the coded fault, got {refused}"
        );
        let listed = listed_ids(&router).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused create must store no configuration, got {listed:?}"
        );

        // Once the account holds that destination the very same create is
        // served, and the configuration reads back from state.
        router.hold_query_log_destinations([destination.arn]);
        let created = ok(&router, "CreateResolverQueryLogConfig", body).await;
        let config = created
            .get("ResolverQueryLogConfig")
            .unwrap_or_else(|| panic!("case {case:?}: the create answers a configuration, got {created}"));
        let id = string_member(config, "Id");
        let fetched = ok(
            &router,
            "GetResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id.clone() }),
        )
        .await;
        let read = fetched
            .get("ResolverQueryLogConfig")
            .unwrap_or_else(|| panic!("case {case:?}: the read answers a configuration, got {fetched}"));
        assert_eq!(
            string_member(read, "Id"),
            id,
            "case {case:?}: the read must answer the configuration the create wrote, got {fetched}"
        );
        assert_eq!(
            string_member(read, "DestinationArn"),
            canonical_query_log_destination(destination.arn),
            "case {case:?}: the stored configuration must name the destination it was created for, \
             in its canonical form, got {fetched}"
        );

        // Each row starts from an empty collection, so the listing assertion
        // above is about the row's own refusal.
        ok(
            &router,
            "DeleteResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id }),
        )
        .await;
    }
}
