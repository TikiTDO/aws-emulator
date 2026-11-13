// SPDX-License-Identifier: CC0-1.0

//! What a configuration set's CloudWatch event destination DELIVERS: the `AWS/SES`
//! datapoints one accepted send leaves behind, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this service
//! and Amazon CloudWatch mounted on ONE engine, which is what makes the store a send
//! publishes into the store CloudWatch reads — and asserts the metric where a client reads
//! it: `cloudwatch:GetMetricStatistics` and `cloudwatch:ListMetrics`.
//!
//! The RULE these cases pin: for each event an accepted send produces, every event
//! destination of the configuration set the message named that is `Enabled`, whose
//! `MatchingEventTypes` names that event's type and that carries a `CloudWatchDestination`
//! counts the event as ONE observation of the `AWS/SES` metric named after that event,
//! dimensioned by EXACTLY the dimensions the destination declared — each one resolved from
//! the send by its own `CloudWatchDimensionConfiguration$DimensionValueSource` ("the
//! location where the Amazon SES API v2 finds the value of a dimension": a message tag of
//! the send, which includes the configuration set SES tags every send through a set with,
//! or a header of the message), falling back to that configuration's
//! `DefaultDimensionValue` where the send supplies no value for it — and by nothing else,
//! so a dimension combination the destination never declared names no series at all.
//!
//! The table spans what the rule generalises over: how many dimensions a destination
//! declares, which `DimensionValueSource` each one names (every value of that enum,
//! `smithy_query(query="shape",shape="DimensionValueSource")`), whether the send supplies a
//! value for the dimension, the case a header name is spelled in, whether the header is one
//! the caller supplied or one SES assembled, which event types the destination matches,
//! whether the destination is on at all, and which events the send's recipient produces
//! (acceptance only, a delivery, a failure, a complaint). The recorded case — the set, a
//! custom message tag and a custom MIME header, matching `SEND` — is one row of it.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The sender every send is from, and the domain identity that authorises it.
const SENDER: &str = "sender@senders.example.com";
const SENDER_DOMAIN: &str = "senders.example.com";

/// The subject every send carries, which is also a header of the message SES assembles —
/// so a declared `EMAIL_HEADER` dimension named `Subject` resolves from it.
const SUBJECT: &str = "a message under test";

/// The namespace Amazon SES publishes a configuration set's email events to.
const NAMESPACE: &str = "AWS/SES";

/// The `x-amz-target` prefix CloudWatch's `awsJson1_0` surface serves (its
/// `OPERATIONS.md` "Wire protocol").
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The period a read lays over the series, in seconds: one minute, the resolution these
/// datapoints are stored at.
const PERIOD_SECS: i64 = 60;

/// How far either side of the send a read's window reaches, in seconds. Generous on
/// purpose: the window is not what any case is about.
const WINDOW_SECS: f64 = 600.0;

/// How many times a read is repeated while waiting for the series a row expects to appear.
///
/// An email event is published ASYNCHRONOUSLY — the message is accepted and answered before
/// any event of it exists — so the series appears on a task of its own. Each pass reads the
/// store again and yields to the runtime; nothing here waits for a SPAN, and the loop stops
/// as soon as what it is waiting for is there.
const READS_WHILE_PUBLISHING: usize = 2_000;

/// How many further reads confirm that what the store holds has settled — so a row that
/// expects one series, or none at all, would also see one it must not have.
const SETTLED_READS: usize = 32;

/// One dimension a destination declares: the name, its `DimensionValueSource` and the
/// default value the configuration carries for a send that supplies none.
#[derive(Clone, Copy)]
struct Dimension {
    name: &'static str,
    source: &'static str,
    default: &'static str,
}

/// One series the `AWS/SES` namespace must hold: the metric name and the dimension set,
/// written out per row rather than derived, so the expectation does not come from the
/// behaviour under test.
#[derive(Clone, Copy)]
struct Series {
    metric: &'static str,
    dimensions: &'static [(&'static str, &'static str)],
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The configuration set the row creates and sends through.
    set: &'static str,
    /// `EventDestinationDefinition$Enabled`.
    enabled: bool,
    /// The event types the destination matches, as the definition names them.
    matching: &'static [&'static str],
    /// The dimensions the destination declares.
    dimensions: &'static [Dimension],
    /// `SendEmailRequest$EmailTags`.
    email_tags: &'static [(&'static str, &'static str)],
    /// `Message$Headers`: the headers the send adds to the message.
    headers: &'static [(&'static str, &'static str)],
    /// The one recipient of the send, whose reaction decides which events it produces.
    recipient: &'static str,
    /// Exactly the series the send must leave in `AWS/SES`, each counting ONE event.
    expected: &'static [Series],
}

/// Every value of the `EventType` enum
/// (`smithy_query(query="shape",shape="EventType")`).
const EVERY_EVENT_TYPE: &[&str] = &[
    "BOUNCE",
    "CLICK",
    "COMPLAINT",
    "DELIVERY",
    "DELIVERY_DELAY",
    "OPEN",
    "REJECT",
    "RENDERING_FAILURE",
    "SEND",
    "SUBSCRIPTION",
];

/// The message tag SES applies to every send made through a configuration set.
const CONFIGURATION_SET_TAG: &str = "ses:configuration-set";

const ROWS: &[Row] = &[
    Row {
        case: "the set, a custom message tag and a custom MIME header, matching SEND",
        set: "cw-events",
        enabled: true,
        matching: &["SEND"],
        dimensions: &[
            Dimension {
                name: CONFIGURATION_SET_TAG,
                source: "MESSAGE_TAG",
                default: "none",
            },
            Dimension {
                name: "campaign",
                source: "MESSAGE_TAG",
                default: "none",
            },
            Dimension {
                name: "X-Custom-Header",
                source: "EMAIL_HEADER",
                default: "none",
            },
        ],
        email_tags: &[("campaign", "spring-launch")],
        headers: &[("X-Custom-Header", "header-of-this-send")],
        recipient: "success@simulator.amazonses.com",
        expected: &[Series {
            metric: "Send",
            dimensions: &[
                (CONFIGURATION_SET_TAG, "cw-events"),
                ("campaign", "spring-launch"),
                ("X-Custom-Header", "header-of-this-send"),
            ],
        }],
    },
    Row {
        case: "a destination that declares only its own dimensions publishes no set \
               dimension",
        set: "custom_only_set",
        enabled: true,
        matching: &["SEND"],
        dimensions: &[
            Dimension {
                name: "campaign",
                source: "MESSAGE_TAG",
                default: "none",
            },
            Dimension {
                name: "X-Custom-Header",
                source: "EMAIL_HEADER",
                default: "none",
            },
        ],
        email_tags: &[("campaign", "summer-sale")],
        headers: &[("X-Custom-Header", "second-header")],
        recipient: "success@simulator.amazonses.com",
        expected: &[Series {
            metric: "Send",
            dimensions: &[
                ("campaign", "summer-sale"),
                ("X-Custom-Header", "second-header"),
            ],
        }],
    },
    Row {
        case: "one declared dimension, and the send supplies its tag",
        set: "s",
        enabled: true,
        matching: &["SEND"],
        dimensions: &[Dimension {
            name: "tier",
            source: "MESSAGE_TAG",
            default: "standard",
        }],
        email_tags: &[("tier", "premium")],
        headers: &[],
        recipient: "success@simulator.amazonses.com",
        expected: &[Series {
            metric: "Send",
            dimensions: &[("tier", "premium")],
        }],
    },
    Row {
        case: "a declared message tag the send does not supply carries its default",
        set: "defaulted-tag-set",
        enabled: true,
        matching: &["SEND"],
        dimensions: &[
            Dimension {
                name: "campaign",
                source: "MESSAGE_TAG",
                default: "none",
            },
            Dimension {
                name: "tier",
                source: "MESSAGE_TAG",
                default: "unset",
            },
        ],
        email_tags: &[("campaign", "autumn-push")],
        headers: &[],
        recipient: "success@simulator.amazonses.com",
        expected: &[Series {
            metric: "Send",
            dimensions: &[("campaign", "autumn-push"), ("tier", "unset")],
        }],
    },
    Row {
        case: "a header dimension resolves whatever case the send spelled the header in, \
               and an absent header carries its default",
        set: "header-case-set",
        enabled: true,
        matching: &["SEND"],
        dimensions: &[
            Dimension {
                name: "X-Custom-Header",
                source: "EMAIL_HEADER",
                default: "none",
            },
            Dimension {
                name: "X-Absent-Header",
                source: "EMAIL_HEADER",
                default: "absent",
            },
        ],
        email_tags: &[],
        headers: &[("x-CUSTOM-header", "spelled-in-another-case")],
        recipient: "success@simulator.amazonses.com",
        expected: &[Series {
            metric: "Send",
            dimensions: &[
                ("X-Absent-Header", "absent"),
                ("X-Custom-Header", "spelled-in-another-case"),
            ],
        }],
    },
    Row {
        case: "a header dimension naming a header SES assembled for the message",
        set: "assembled-header-set",
        enabled: true,
        matching: &["SEND"],
        dimensions: &[Dimension {
            name: "Subject",
            source: "EMAIL_HEADER",
            default: "no-subject",
        }],
        email_tags: &[],
        headers: &[],
        recipient: "success@simulator.amazonses.com",
        expected: &[Series {
            metric: "Send",
            dimensions: &[("Subject", SUBJECT)],
        }],
    },
    Row {
        case: "a LINK_TAG dimension carries its default, since no link of this send was \
               followed",
        set: "link-tag-set",
        enabled: true,
        matching: &["SEND"],
        dimensions: &[
            Dimension {
                name: CONFIGURATION_SET_TAG,
                source: "MESSAGE_TAG",
                default: "none",
            },
            Dimension {
                name: "link-id",
                source: "LINK_TAG",
                default: "no-link",
            },
        ],
        email_tags: &[],
        headers: &[],
        recipient: "success@simulator.amazonses.com",
        expected: &[Series {
            metric: "Send",
            dimensions: &[
                (CONFIGURATION_SET_TAG, "link-tag-set"),
                ("link-id", "no-link"),
            ],
        }],
    },
    Row {
        case: "a destination matching only DELIVERY counts the delivery and not the \
               acceptance",
        set: "delivery-only-set",
        enabled: true,
        matching: &["DELIVERY"],
        dimensions: &[Dimension {
            name: CONFIGURATION_SET_TAG,
            source: "MESSAGE_TAG",
            default: "none",
        }],
        email_tags: &[],
        headers: &[],
        recipient: "success@simulator.amazonses.com",
        expected: &[Series {
            metric: "Delivery",
            dimensions: &[(CONFIGURATION_SET_TAG, "delivery-only-set")],
        }],
    },
    Row {
        case: "a destination matching every event type, for a recipient that rejects the \
               message",
        set: "every-type-bounce-set",
        enabled: true,
        matching: EVERY_EVENT_TYPE,
        dimensions: &[Dimension {
            name: "campaign",
            source: "MESSAGE_TAG",
            default: "none",
        }],
        email_tags: &[("campaign", "winter-tease")],
        headers: &[],
        recipient: "bounce@simulator.amazonses.com",
        expected: &[
            Series {
                metric: "Bounce",
                dimensions: &[("campaign", "winter-tease")],
            },
            Series {
                metric: "Send",
                dimensions: &[("campaign", "winter-tease")],
            },
        ],
    },
    Row {
        case: "a destination matching every event type, for a recipient that reports the \
               message",
        set: "every-type-complaint-set",
        enabled: true,
        matching: EVERY_EVENT_TYPE,
        dimensions: &[Dimension {
            name: CONFIGURATION_SET_TAG,
            source: "MESSAGE_TAG",
            default: "none",
        }],
        email_tags: &[],
        headers: &[],
        recipient: "complaint@simulator.amazonses.com",
        expected: &[
            Series {
                metric: "Complaint",
                dimensions: &[(CONFIGURATION_SET_TAG, "every-type-complaint-set")],
            },
            Series {
                metric: "Delivery",
                dimensions: &[(CONFIGURATION_SET_TAG, "every-type-complaint-set")],
            },
            Series {
                metric: "Send",
                dimensions: &[(CONFIGURATION_SET_TAG, "every-type-complaint-set")],
            },
        ],
    },
    Row {
        case: "a destination that is off counts nothing",
        set: "switched-off-set",
        enabled: false,
        matching: &["SEND", "DELIVERY"],
        dimensions: &[Dimension {
            name: CONFIGURATION_SET_TAG,
            source: "MESSAGE_TAG",
            default: "none",
        }],
        email_tags: &[],
        headers: &[],
        recipient: "success@simulator.amazonses.com",
        expected: &[],
    },
    Row {
        case: "a destination matching an event type this send never produces counts \
               nothing",
        set: "unmatched-type-set",
        enabled: true,
        matching: &["COMPLAINT", "OPEN", "CLICK"],
        dimensions: &[Dimension {
            name: CONFIGURATION_SET_TAG,
            source: "MESSAGE_TAG",
            default: "none",
        }],
        email_tags: &[],
        headers: &[],
        recipient: "success@simulator.amazonses.com",
        expected: &[],
    },
];

/// One answered request: its status and parsed body.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// A string at `pointer`, or `""` when the body carries nothing there.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator under test: this service and CloudWatch on ONE engine, the shape
/// `core/router/src/lib.rs` mounts every service in.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// One call on this service's restJson1 surface.
    async fn ses(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.{REGION}.amazonaws.com{path}"))
            .header("host", format!("email.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAEMULATOR0000000/20260101/{REGION}/ses/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// The same call, requiring the 200 the operation's output shape declares.
    async fn ses_ok(&self, case: &str, method: &str, path: &str, body: Option<Value>) -> Reply {
        let reply = self.ses(method, path, body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: {method} {path} must be served, got {} {}",
            reply.status, reply.body
        );
        reply
    }

    /// One CloudWatch call on its `awsJson1_0` surface, requiring a 200.
    async fn cloudwatch(&self, case: &str, operation: &str, body: &Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("monitoring.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header(
                "x-amz-target",
                format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAEMULATOR0000000/20260101/{REGION}/monitoring/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize body"),
            ))
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed: Value = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, 200,
            "case {case:?}: cloudwatch:{operation} must be served, got {status} {parsed}"
        );
        parsed
    }

    /// The account with a verified sending domain, so every send below is authorised.
    async fn given_verified_sender(&self, case: &str) {
        self.ses_ok(
            case,
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": SENDER_DOMAIN})),
        )
        .await;
        // Bring the verification transition the clock owes the identity into the past, so
        // a send does not race the span.
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let past = SystemTime::now()
            .checked_sub(Duration::from_secs(1))
            .expect("an instant before now");
        for record in store.identities.values_mut() {
            record.verification_due_at = past;
        }
        store.settle(SystemTime::now());
    }

    /// A configuration set carrying one CloudWatch event destination.
    async fn given_destination(
        &self,
        case: &str,
        set: &str,
        enabled: bool,
        matching: &[&str],
        dimensions: &[Dimension],
    ) {
        self.ses_ok(
            case,
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({"ConfigurationSetName": set})),
        )
        .await;
        let configurations: Vec<Value> = dimensions
            .iter()
            .map(|dimension| {
                json!({
                    "DimensionName": dimension.name,
                    "DimensionValueSource": dimension.source,
                    "DefaultDimensionValue": dimension.default,
                })
            })
            .collect();
        self.ses_ok(
            case,
            "POST",
            &format!("/v2/email/configuration-sets/{set}/event-destinations"),
            Some(json!({
                "EventDestinationName": "sim2real-cw",
                "EventDestination": {
                    "Enabled": enabled,
                    "MatchingEventTypes": matching,
                    "CloudWatchDestination": {"DimensionConfigurations": configurations},
                },
            })),
        )
        .await;
    }

    /// `SendEmail` through `set`, answering the `MessageId` it minted.
    async fn send(
        &self,
        case: &str,
        set: &str,
        recipient: &str,
        email_tags: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> String {
        let mut simple = json!({
            "Subject": {"Data": SUBJECT},
            "Body": {"Text": {"Data": "hello"}},
        });
        if !headers.is_empty() {
            simple["Headers"] = Value::Array(
                headers
                    .iter()
                    .map(|(name, value)| json!({"Name": name, "Value": value}))
                    .collect(),
            );
        }
        let request = json!({
            "FromEmailAddress": SENDER,
            "Destination": {"ToAddresses": [recipient]},
            "ConfigurationSetName": set,
            "Content": {"Simple": simple},
            "EmailTags": email_tags
                .iter()
                .map(|(name, value)| json!({"Name": name, "Value": value}))
                .collect::<Vec<Value>>(),
        });
        let reply = self
            .ses_ok(case, "POST", "/v2/email/outbound-emails", Some(request))
            .await;
        let message_id = reply.text("/MessageId");
        assert!(
            !message_id.is_empty(),
            "case {case:?}: an accepted send must answer a MessageId, got {}",
            reply.body
        );
        message_id
    }

    /// Every series the `AWS/SES` namespace holds, as `ListMetrics` reports them: each the
    /// metric name and its dimension set, both ordered so a comparison is stable.
    async fn listed_series(&self, case: &str) -> Vec<(String, Vec<(String, String)>)> {
        let answered = self
            .cloudwatch(case, "ListMetrics", &json!({"Namespace": NAMESPACE}))
            .await;
        let mut series: Vec<(String, Vec<(String, String)>)> = answered
            .get("Metrics")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|metric| (text(metric, "MetricName"), dimensions_of(metric)))
            .collect();
        series.sort();
        series
    }

    /// The datapoints `GetMetricStatistics` answers for the series `metric`/`dimensions`
    /// over the window around the send.
    async fn datapoints(
        &self,
        case: &str,
        metric: &str,
        dimensions: &[(&str, &str)],
    ) -> Vec<Value> {
        let now = now_secs();
        let answered = self
            .cloudwatch(
                case,
                "GetMetricStatistics",
                &json!({
                    "Namespace": NAMESPACE,
                    "MetricName": metric,
                    "Dimensions": dimensions
                        .iter()
                        .map(|(name, value)| json!({"Name": name, "Value": value}))
                        .collect::<Vec<Value>>(),
                    "StartTime": now - WINDOW_SECS,
                    "EndTime": now + WINDOW_SECS,
                    "Period": PERIOD_SECS,
                    "Statistics": ["Sum"],
                }),
            )
            .await;
        answered
            .get("Datapoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `Sum` over every datapoint of one series' window: how many events it counted.
    async fn sum(&self, case: &str, metric: &str, dimensions: &[(&str, &str)]) -> f64 {
        self.datapoints(case, metric, dimensions)
            .await
            .iter()
            .filter_map(|point| point.get("Sum").and_then(Value::as_f64))
            .sum()
    }
}

/// A string member of a reported metric, or `""` when absent.
fn text(metric: &Value, member: &str) -> String {
    metric
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The dimension set of a reported metric, ordered by name.
fn dimensions_of(metric: &Value) -> Vec<(String, String)> {
    let mut dimensions: Vec<(String, String)> = metric
        .get("Dimensions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|dimension| (text(dimension, "Name"), text(dimension, "Value")))
        .collect();
    dimensions.sort();
    dimensions
}

/// Wall-clock now as epoch seconds, the clock a metric window is measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// The series a row expects, in the order [`Emulator::listed_series`] reports them.
fn expected_series(row: &Row) -> Vec<(String, Vec<(String, String)>)> {
    let mut series: Vec<(String, Vec<(String, String)>)> = row
        .expected
        .iter()
        .map(|wanted| {
            let mut dimensions: Vec<(String, String)> = wanted
                .dimensions
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect();
            dimensions.sort();
            (wanted.metric.to_string(), dimensions)
        })
        .collect();
    series.sort();
    series
}

/// Read the namespace until the series `row` expects have all appeared, and then until
/// what it holds has settled — so a row that expects fewer series than the store ends up
/// holding fails on the comparison rather than on a race.
async fn settled_series(
    emulator: &Emulator,
    row: &Row,
) -> Vec<(String, Vec<(String, String)>)> {
    let wanted = row.expected.len();
    let mut listed = emulator.listed_series(row.case).await;
    for _ in 0..READS_WHILE_PUBLISHING {
        if listed.len() >= wanted {
            break;
        }
        ::tokio::task::yield_now().await;
        listed = emulator.listed_series(row.case).await;
    }
    for _ in 0..SETTLED_READS {
        ::tokio::task::yield_now().await;
        let again = emulator.listed_series(row.case).await;
        assert_eq!(
            again, listed,
            "case {:?}: the AWS/SES namespace kept changing: {listed:?} then {again:?}",
            row.case
        );
    }
    listed
}

#[tokio::test]
async fn a_send_counts_one_datapoint_per_event_under_exactly_the_declared_dimensions() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.given_verified_sender(case).await;
        emulator
            .given_destination(case, row.set, row.enabled, row.matching, row.dimensions)
            .await;

        // Nothing is in the namespace before the send: the destination alone publishes
        // nothing.
        assert!(
            emulator.listed_series(case).await.is_empty(),
            "case {case:?}: attaching a destination must publish no datapoint, got {:?}",
            emulator.listed_series(case).await
        );

        emulator
            .send(case, row.set, row.recipient, row.email_tags, row.headers)
            .await;

        let listed = settled_series(&emulator, row).await;
        assert_eq!(
            listed,
            expected_series(row),
            "case {case:?}: expected exactly the declared series in AWS/SES, got {listed:?}"
        );

        // Each of them counts the ONE event it is about.
        for wanted in row.expected {
            let points = emulator
                .datapoints(case, wanted.metric, wanted.dimensions)
                .await;
            assert_eq!(
                points.len(),
                1,
                "case {case:?}: expected one {:?} datapoint for {:?}, got {points:?}",
                wanted.metric,
                wanted.dimensions
            );
            let sum = emulator.sum(case, wanted.metric, wanted.dimensions).await;
            assert_eq!(
                sum, 1.0,
                "case {case:?}: expected Sum 1 for {:?} over {:?}, got {sum}",
                wanted.metric, wanted.dimensions
            );
        }
    }
}

#[tokio::test]
async fn a_dimension_the_destination_did_not_declare_has_no_series_at_all() {
    // The mirror image of the positive half: a destination that declares only its own
    // dimensions publishes a series for exactly those, and a read that adds a dimension it
    // never declared — the configuration set SES tags every send with, or a name nothing
    // configured — finds NO series rather than one defaulted to its `DefaultDimensionValue`.
    let case = "a destination declaring only its own dimensions";
    let set = "undeclared-probe-set";
    let declared: &[(&str, &str)] = &[("campaign", "probe-campaign")];
    let emulator = Emulator::new();
    emulator.given_verified_sender(case).await;
    emulator
        .given_destination(
            case,
            set,
            true,
            &["SEND"],
            &[Dimension {
                name: "campaign",
                source: "MESSAGE_TAG",
                default: "none",
            }],
        )
        .await;
    emulator
        .send(
            case,
            set,
            "success@simulator.amazonses.com",
            &[("campaign", "probe-campaign")],
            &[],
        )
        .await;

    let row = Row {
        case,
        set,
        enabled: true,
        matching: &["SEND"],
        dimensions: &[],
        email_tags: &[],
        headers: &[],
        recipient: "success@simulator.amazonses.com",
        expected: &[Series {
            metric: "Send",
            dimensions: &[("campaign", "probe-campaign")],
        }],
    };
    let listed = settled_series(&emulator, &row).await;
    assert_eq!(
        listed,
        expected_series(&row),
        "case {case:?}: expected exactly the declared series, got {listed:?}"
    );

    // The declared combination is there and counts the send.
    let sum = emulator.sum(case, "Send", declared).await;
    assert_eq!(
        sum, 1.0,
        "case {case:?}: expected Sum 1 for the declared dimensions, got {sum}"
    );

    // Every combination the destination did not declare has no datapoint at all, and an
    // absence stays an absence over further reads.
    let undeclared: [&[(&str, &str)]; 3] = [
        &[("campaign", "probe-campaign"), (CONFIGURATION_SET_TAG, set)],
        &[(CONFIGURATION_SET_TAG, set)],
        &[("campaign", "probe-campaign"), ("tier", "none")],
    ];
    for probe in undeclared {
        for _ in 0..SETTLED_READS {
            let points = emulator.datapoints(case, "Send", probe).await;
            assert!(
                points.is_empty(),
                "case {case:?}: {probe:?} is not a combination this destination declared, so \
                 it must name no series, got {points:?}"
            );
            ::tokio::task::yield_now().await;
        }
    }
}

#[tokio::test]
async fn each_send_counts_once_more_under_the_same_dimensions() {
    // One observation per EVENT: three sends through the same destination with the same
    // tags count three, in the one series their dimension set identifies.
    let case = "three sends through one destination";
    let set = "repeated-sends-set";
    let dimensions: &[(&str, &str)] = &[(CONFIGURATION_SET_TAG, "repeated-sends-set")];
    let sends = 3;
    let emulator = Emulator::new();
    emulator.given_verified_sender(case).await;
    emulator
        .given_destination(
            case,
            set,
            true,
            &["SEND"],
            &[Dimension {
                name: CONFIGURATION_SET_TAG,
                source: "MESSAGE_TAG",
                default: "none",
            }],
        )
        .await;
    for _ in 0..sends {
        emulator
            .send(case, set, "success@simulator.amazonses.com", &[], &[])
            .await;
    }

    let mut counted = 0.0;
    for _ in 0..READS_WHILE_PUBLISHING {
        counted = emulator.sum(case, "Send", dimensions).await;
        if counted >= f64::from(sends) {
            break;
        }
        ::tokio::task::yield_now().await;
    }
    assert_eq!(
        counted,
        f64::from(sends),
        "case {case:?}: expected the series to count every send, got {counted}"
    );

    // Reading the series does not change it.
    let read_again = emulator.sum(case, "Send", dimensions).await;
    assert_eq!(
        read_again, counted,
        "case {case:?}: a second read counted {read_again}, not the first read's {counted}"
    );
}
