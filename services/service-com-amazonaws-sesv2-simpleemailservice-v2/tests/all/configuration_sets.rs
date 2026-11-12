// SPDX-License-Identifier: CC0-1.0

//! The configuration-set surface, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! The rule these cases pin: a create of a named resource PERSISTS the configuration the
//! request carried and answers an empty 200, a read of that name reports exactly that
//! configuration (with the service's documented default for every option the request did
//! not configure) and leaves it unchanged, and a call the service cannot serve is refused
//! with the modelled error for its reason — a duplicate name with `AlreadyExistsException`,
//! a name outside the documented character-and-length rule or an enum member outside its
//! modelled value set with `BadRequestException`, and a name the account does not hold
//! with `NotFoundException`.
//!
//! The tables below span what the rule generalises over: which option objects the create
//! carries (none, each one, several together), the values inside them (both `TlsPolicy`
//! values, every `HttpsPolicy` value, both suppression reasons, both suppression scopes,
//! both feature statuses, both delivery bounds), the form of the name (one character, the
//! longest allowed, hyphens, underscores, digits) and the refusal legs.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
///
/// Shared with the sibling suites of this test target, so every case reads a reply the
/// same way.
pub(crate) struct Reply {
    pub(crate) status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    pub(crate) fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    pub(crate) fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A string member of the body, or `""` when absent.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A boolean at `pointer`, where absent reads as `false`: the modelled default of
    /// every boolean on a configuration set is `false`, so a serializer that omits a
    /// defaulted member and one that writes it out agree here.
    pub(crate) fn flag(&self, pointer: &str) -> bool {
        self.body
            .pointer(pointer)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// A member of the body, or `Value::Null` when absent.
    pub(crate) fn member(&self, member: &str) -> Value {
        self.body.get(member).cloned().unwrap_or(Value::Null)
    }

    /// The value at `pointer`, or `None` when the body carries nothing there.
    pub(crate) fn pointer(&self, pointer: &str) -> Option<Value> {
        self.body.pointer(pointer).cloned()
    }

    /// Whether the body carries no members at all: what an operation whose output shape
    /// declares none answers with.
    pub(crate) fn is_empty_body(&self) -> bool {
        match &self.body {
            Value::Null => true,
            Value::Object(members) => members.is_empty(),
            _ => false,
        }
    }
}

/// The emulated service behind its router.
///
/// Shared with the sibling suites of this test target, so every case drives the same
/// wire path.
pub(crate) struct Ses {
    router: MultiServiceRouter,
    /// The engine the service is mounted on, kept so a suite can put the account in a
    /// state no wire call can reach in one step.
    engine: Arc<engine::InMemoryEngine>,
}

impl Ses {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The same emulator with Amazon SNS mounted beside this service on the ONE engine —
    /// the shape `core/router/src/lib.rs` mounts every service in — for a suite whose
    /// subject is a resource of that service, which this one reaches through the
    /// in-process callback.
    pub(crate) fn with_sns() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// The same emulator with AWS Identity and Access Management mounted beside this
    /// service on the ONE engine, for a suite whose subject is a ROLE this service acts as:
    /// the role's trust policy is created and read through the IAM API, which this service
    /// reaches through the in-process callback.
    pub(crate) fn with_iam() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// The in-process callback into this same emulator: the seam a handler reaches
    /// another mounted service through, so a suite's setup reaches that service exactly
    /// the way the handler does.
    pub(crate) fn callback(&self) -> smithy_server_runtime::EmbeddedEmulator {
        smithy_server_runtime::EmbeddedEmulator::from_router(self.router.clone())
    }

    /// Grant this account production access, which is what takes it out of the sandbox.
    ///
    /// It is set on the one account record every account operation reads and writes (the
    /// record `GetAccount` reports `ProductionAccessEnabled` from), because asking for it
    /// over the wire opens a support case whose decision only falls due later: a suite
    /// that needs a call a sandboxed account is refused grants it here.
    pub(crate) fn grant_production_access(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        state.lock().account.production_access_enabled = true;
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        self.call_in("us-east-1", method, path, body).await
    }

    /// The same call, signed for `region`: the SigV4 credential scope is what names the
    /// Region a request executes in, so a case about regional behaviour varies it here.
    pub(crate) async fn call_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let host = format!("email.{region}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{region}/ses/aws4_request, SignedHeaders=host, Signature=00"),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    pub(crate) async fn create_set(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/configuration-sets", Some(body))
            .await
    }

    pub(crate) async fn get_set(&self, name: &str) -> Reply {
        self.call("GET", &format!("/v2/email/configuration-sets/{name}"), None)
            .await
    }
}

/// One row of the create-and-read table: the request body, and the configuration a read
/// of the stored set must report afterwards.
struct ConfigurationSetCase {
    case: &'static str,
    name: String,
    body: Value,
    /// `DeliveryOptions` as the read must report it: `Value::Null` where the request
    /// configured no delivery options at all, so the set carries none of its own, and
    /// otherwise exactly the members the request carried.
    delivery_options: Value,
    /// `ReputationOptions$ReputationMetricsEnabled`.
    /// What the request configured, or the state a set nobody configured starts in,
    /// which is collecting the metrics.
    reputation_metrics_enabled: bool,
    /// `SendingOptions$SendingEnabled`.
    sending_enabled: bool,
    /// `TrackingOptions`, `SuppressionOptions` and `VdmOptions` as the read must report
    /// them: `Value::Null` where the request configured none, so the set follows the
    /// account's own preferences instead of holding one of its own.
    tracking_options: Value,
    suppression_options: Value,
    vdm_options: Value,
    /// `Tags`: the tag set the create supplied, as the list a read reports — an EMPTY
    /// list for a set created with none.
    tags: Value,
}

/// The rows of the create-and-read table.
///
/// Built at run time because two rows carry a generated name (one character, and the
/// longest a `ConfigurationSetName` may be).
fn configuration_set_cases() -> Vec<ConfigurationSetCase> {
    // The documented length bound of `ConfigurationSetName`: "The name can contain up to
    // 64 alphanumeric characters, including letters, numbers, hyphens (-) and underscores
    // (_) only."
    let longest_name = "x".repeat(64);
    vec![
        ConfigurationSetCase {
            case: "nothing configured, one-character name",
            name: "a".to_string(),
            body: json!({"ConfigurationSetName": "a"}),
            delivery_options: Value::Null,
            reputation_metrics_enabled: true,
            sending_enabled: true,
            tracking_options: Value::Null,
            suppression_options: Value::Null,
            vdm_options: Value::Null,
            tags: json!([]),
        },
        ConfigurationSetCase {
            case: "nothing configured, longest allowed name",
            name: longest_name.clone(),
            body: json!({"ConfigurationSetName": longest_name}),
            delivery_options: Value::Null,
            reputation_metrics_enabled: true,
            sending_enabled: true,
            tracking_options: Value::Null,
            suppression_options: Value::Null,
            vdm_options: Value::Null,
            tags: json!([]),
        },
        ConfigurationSetCase {
            case: "tracking domain only, hyphenated name",
            name: "tracking-only-set".to_string(),
            body: json!({
                "ConfigurationSetName": "tracking-only-set",
                "TrackingOptions": {"CustomRedirectDomain": "click.example.com"},
            }),
            delivery_options: Value::Null,
            reputation_metrics_enabled: true,
            sending_enabled: true,
            tracking_options: json!({"CustomRedirectDomain": "click.example.com"}),
            suppression_options: Value::Null,
            vdm_options: Value::Null,
            tags: json!([]),
        },
        ConfigurationSetCase {
            // `HttpsPolicy` value set from the model's own enum (smithy_query
            // shape="HttpsPolicy"): OPTIONAL, REQUIRE, REQUIRE_OPEN_ONLY.
            case: "tracking domain with REQUIRE_OPEN_ONLY https policy",
            name: "tracking_https_set".to_string(),
            body: json!({
                "ConfigurationSetName": "tracking_https_set",
                "TrackingOptions": {
                    "CustomRedirectDomain": "links.example.org",
                    "HttpsPolicy": "REQUIRE_OPEN_ONLY",
                },
            }),
            delivery_options: Value::Null,
            reputation_metrics_enabled: true,
            sending_enabled: true,
            tracking_options: json!({
                "CustomRedirectDomain": "links.example.org",
                "HttpsPolicy": "REQUIRE_OPEN_ONLY",
            }),
            suppression_options: Value::Null,
            vdm_options: Value::Null,
            tags: json!([]),
        },
        ConfigurationSetCase {
            // `MaxDeliverySeconds` range from the model (smithy_query
            // shape="MaxDeliverySeconds"): 300 to 50400. This row takes the lower bound.
            case: "TLS required, delivery bounded at the published minimum",
            name: "tls-require-300".to_string(),
            body: json!({
                "ConfigurationSetName": "tls-require-300",
                "DeliveryOptions": {"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 300},
            }),
            delivery_options: json!({"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 300}),
            reputation_metrics_enabled: true,
            sending_enabled: true,
            tracking_options: Value::Null,
            suppression_options: Value::Null,
            vdm_options: Value::Null,
            tags: json!([]),
        },
        ConfigurationSetCase {
            // ... and this row the published maximum, with the other `TlsPolicy` value.
            case: "TLS optional, delivery bounded at the published maximum",
            name: "tls-optional-50400".to_string(),
            body: json!({
                "ConfigurationSetName": "tls-optional-50400",
                "DeliveryOptions": {"TlsPolicy": "OPTIONAL", "MaxDeliverySeconds": 50400},
            }),
            delivery_options: json!({"TlsPolicy": "OPTIONAL", "MaxDeliverySeconds": 50400}),
            reputation_metrics_enabled: true,
            sending_enabled: true,
            tracking_options: Value::Null,
            suppression_options: Value::Null,
            vdm_options: Value::Null,
            tags: json!([]),
        },
        ConfigurationSetCase {
            case: "reputation metrics on and sending paused",
            name: "reputation-on-sending-off".to_string(),
            body: json!({
                "ConfigurationSetName": "reputation-on-sending-off",
                "ReputationOptions": {"ReputationMetricsEnabled": true},
                "SendingOptions": {"SendingEnabled": false},
            }),
            delivery_options: Value::Null,
            reputation_metrics_enabled: true,
            sending_enabled: false,
            tracking_options: Value::Null,
            suppression_options: Value::Null,
            vdm_options: Value::Null,
            tags: json!([]),
        },
        ConfigurationSetCase {
            case: "reputation metrics explicitly turned off",
            name: "reputation-off".to_string(),
            body: json!({
                "ConfigurationSetName": "reputation-off",
                "ReputationOptions": {"ReputationMetricsEnabled": false},
            }),
            delivery_options: Value::Null,
            reputation_metrics_enabled: false,
            sending_enabled: true,
            tracking_options: Value::Null,
            suppression_options: Value::Null,
            vdm_options: Value::Null,
            tags: json!([]),
        },
        ConfigurationSetCase {
            // `SuppressionListReason` and `SuppressionListScope` value sets from the
            // model (smithy_query shape="SuppressionListReasons,SuppressionListScope"):
            // BOUNCE, COMPLAINT and ACCOUNT, TENANT.
            case: "both suppression reasons, account scope",
            name: "suppress-both-account".to_string(),
            body: json!({
                "ConfigurationSetName": "suppress-both-account",
                "SuppressionOptions": {
                    "SuppressedReasons": ["BOUNCE", "COMPLAINT"],
                    "SuppressionScope": "ACCOUNT",
                },
            }),
            delivery_options: Value::Null,
            reputation_metrics_enabled: true,
            sending_enabled: true,
            tracking_options: Value::Null,
            suppression_options: json!({
                "SuppressedReasons": ["BOUNCE", "COMPLAINT"],
                "SuppressionScope": "ACCOUNT",
            }),
            vdm_options: Value::Null,
            tags: json!([]),
        },
        ConfigurationSetCase {
            case: "one suppression reason, tenant scope",
            name: "suppress_complaint_tenant".to_string(),
            body: json!({
                "ConfigurationSetName": "suppress_complaint_tenant",
                "SuppressionOptions": {
                    "SuppressedReasons": ["COMPLAINT"],
                    "SuppressionScope": "TENANT",
                },
            }),
            delivery_options: Value::Null,
            reputation_metrics_enabled: true,
            sending_enabled: true,
            tracking_options: Value::Null,
            suppression_options: json!({
                "SuppressedReasons": ["COMPLAINT"],
                "SuppressionScope": "TENANT",
            }),
            vdm_options: Value::Null,
            tags: json!([]),
        },
        ConfigurationSetCase {
            // `FeatureStatus` value set from the model (smithy_query
            // shape="FeatureStatus"): ENABLED, DISABLED. Both appear in this row.
            case: "VDM dashboard enabled and guardian disabled",
            name: "vdm-set-9".to_string(),
            body: json!({
                "ConfigurationSetName": "vdm-set-9",
                "VdmOptions": {
                    "DashboardOptions": {"EngagementMetrics": "ENABLED"},
                    "GuardianOptions": {"OptimizedSharedDelivery": "DISABLED"},
                },
            }),
            delivery_options: Value::Null,
            reputation_metrics_enabled: true,
            sending_enabled: true,
            tracking_options: Value::Null,
            suppression_options: Value::Null,
            vdm_options: json!({
                "DashboardOptions": {"EngagementMetrics": "ENABLED"},
                "GuardianOptions": {"OptimizedSharedDelivery": "DISABLED"},
            }),
            tags: json!([]),
        },
        ConfigurationSetCase {
            case: "every option object together, tagged, digits in the name",
            name: "everything_00_set".to_string(),
            body: json!({
                "ConfigurationSetName": "everything_00_set",
                "TrackingOptions": {
                    "CustomRedirectDomain": "track.example.net",
                    "HttpsPolicy": "REQUIRE",
                },
                "DeliveryOptions": {"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 1800},
                "ReputationOptions": {"ReputationMetricsEnabled": true},
                "SendingOptions": {"SendingEnabled": true},
                "SuppressionOptions": {"SuppressedReasons": ["BOUNCE"]},
                "VdmOptions": {"DashboardOptions": {"EngagementMetrics": "DISABLED"}},
                "Tags": [
                    {"Key": "env", "Value": "test"},
                    {"Key": "owner", "Value": "configuration-sets"},
                ],
            }),
            delivery_options: json!({"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 1800}),
            reputation_metrics_enabled: true,
            sending_enabled: true,
            tracking_options: json!({
                "CustomRedirectDomain": "track.example.net",
                "HttpsPolicy": "REQUIRE",
            }),
            suppression_options: json!({"SuppressedReasons": ["BOUNCE"]}),
            vdm_options: json!({"DashboardOptions": {"EngagementMetrics": "DISABLED"}}),
            tags: json!([
                {"Key": "env", "Value": "test"},
                {"Key": "owner", "Value": "configuration-sets"},
            ]),
        },
    ]
}

#[tokio::test]
async fn create_persists_the_configuration_and_get_reads_it_back() {
    let ses = Ses::new();
    let cases = configuration_set_cases();

    for case in &cases {
        let created = ses.create_set(case.body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateConfigurationSet, got {} {:?}",
            case.case, created.status, created.body
        );
        assert!(
            created.is_empty_body(),
            "case {:?}: CreateConfigurationSetResponse declares no members, got {:?}",
            case.case,
            created.body
        );

        let read = ses.get_set(&case.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetConfigurationSet, got {} {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            read.text("ConfigurationSetName"),
            case.name,
            "case {:?}: expected the stored name {:?}, got {:?}",
            case.case,
            case.name,
            read.body
        );
        assert_eq!(
            read.member("DeliveryOptions"),
            case.delivery_options,
            "case {:?}: expected DeliveryOptions {:?}, got {:?}",
            case.case,
            case.delivery_options,
            read.body
        );
        assert_eq!(
            read.flag("/ReputationOptions/ReputationMetricsEnabled"),
            case.reputation_metrics_enabled,
            "case {:?}: expected ReputationMetricsEnabled {}, got {:?}",
            case.case,
            case.reputation_metrics_enabled,
            read.body
        );
        // `LastFreshStart` is "the date and time when the reputation metrics were last
        // given a fresh start", which for a set whose sending was never disabled and
        // re-enabled is when the set was created — so every stored set reports one.
        assert!(
            read.body.pointer("/ReputationOptions/LastFreshStart").is_some(),
            "case {:?}: expected a LastFreshStart on the stored set, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.flag("/SendingOptions/SendingEnabled"),
            case.sending_enabled,
            "case {:?}: expected SendingEnabled {}, got {:?}",
            case.case,
            case.sending_enabled,
            read.body
        );
        assert_eq!(
            read.member("TrackingOptions"),
            case.tracking_options,
            "case {:?}: expected TrackingOptions {:?}, got {:?}",
            case.case,
            case.tracking_options,
            read.body
        );
        assert_eq!(
            read.member("SuppressionOptions"),
            case.suppression_options,
            "case {:?}: expected SuppressionOptions {:?}, got {:?}",
            case.case,
            case.suppression_options,
            read.body
        );
        assert_eq!(
            read.member("VdmOptions"),
            case.vdm_options,
            "case {:?}: expected VdmOptions {:?}, got {:?}",
            case.case,
            case.vdm_options,
            read.body
        );
        assert_eq!(
            read.member("Tags"),
            case.tags,
            "case {:?}: expected Tags {:?}, got {:?}",
            case.case,
            case.tags,
            read.body
        );

        // A read reports the set without changing it: a second read answers exactly the
        // same configuration, timestamp included.
        let again = ses.get_set(&case.name).await;
        assert_eq!(
            (again.status, &again.body),
            (200, &read.body),
            "case {:?}: a read must not change what a later reader sees, got {} {:?}",
            case.case,
            again.status,
            again.body
        );

        // The name identifies the set, so a second create under it is refused rather
        // than replacing the stored configuration.
        let duplicate = ses.create_set(case.body.clone()).await;
        assert_eq!(
            (duplicate.status, duplicate.error_code().as_str()),
            (400, "AlreadyExistsException"),
            "case {:?}: expected 400 AlreadyExistsException on a duplicate create, got {} {:?}",
            case.case,
            duplicate.status,
            duplicate.body
        );
        assert!(
            duplicate.message().contains(&case.name),
            "case {:?}: the refusal must name the configuration set, got {:?}",
            case.case,
            duplicate.message()
        );
    }

    // Every set is still readable as its own configuration after the others were
    // created: one store, keyed by name, not a single last-write-wins record.
    for case in &cases {
        let read = ses.get_set(&case.name).await;
        assert_eq!(
            (read.status, read.text("ConfigurationSetName")),
            (200, case.name.clone()),
            "case {:?}: expected the set to survive the later creates, got {} {:?}",
            case.case,
            read.status,
            read.body
        );
        assert_eq!(
            read.member("DeliveryOptions"),
            case.delivery_options,
            "case {:?}: expected DeliveryOptions {:?} after the later creates, got {:?}",
            case.case,
            case.delivery_options,
            read.body
        );
    }
}

/// One row of the name table: the name, and whether SES accepts it.
struct NameCase {
    case: &'static str,
    name: String,
    accepted: bool,
}

/// The rows of the name table.
///
/// The rule is `ConfigurationSetName`'s own documentation — "The name can contain up to 64
/// alphanumeric characters, including letters, numbers, hyphens (-) and underscores (_)
/// only" — so the rows walk each allowed character class, the published length bound and
/// one character past it, and characters from outside the allowed set.
fn name_cases() -> Vec<NameCase> {
    vec![
        NameCase {
            case: "one letter",
            name: "s".to_string(),
            accepted: true,
        },
        NameCase {
            case: "one digit",
            name: "7".to_string(),
            accepted: true,
        },
        NameCase {
            case: "mixed case, digits, hyphen and underscore",
            name: "Set-02_Alpha".to_string(),
            accepted: true,
        },
        NameCase {
            case: "the published length bound",
            name: "n".repeat(64),
            accepted: true,
        },
        NameCase {
            case: "one character past the published length bound",
            name: "n".repeat(65),
            accepted: false,
        },
        NameCase {
            case: "a dot is not an allowed character",
            name: "set.with.dots".to_string(),
            accepted: false,
        },
        NameCase {
            case: "a plus is not an allowed character",
            name: "set+plus".to_string(),
            accepted: false,
        },
        NameCase {
            case: "a non-ASCII letter is not alphanumeric for this rule",
            name: "sét".to_string(),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn a_name_outside_the_documented_rule_is_refused() {
    let ses = Ses::new();

    for case in name_cases() {
        let created = ses.create_set(json!({"ConfigurationSetName": case.name})).await;
        if case.accepted {
            assert_eq!(
                created.status, 200,
                "case {:?}: expected 200 for name {:?}, got {} {:?}",
                case.case, case.name, created.status, created.body
            );
            let read = ses.get_set(&case.name).await;
            assert_eq!(
                (read.status, read.text("ConfigurationSetName")),
                (200, case.name.clone()),
                "case {:?}: expected the accepted name to read back, got {} {:?}",
                case.case,
                read.status,
                read.body
            );
            continue;
        }
        assert_eq!(
            (created.status, created.error_code().as_str()),
            (400, "BadRequestException"),
            "case {:?}: expected 400 BadRequestException for name {:?}, got {} {:?}",
            case.case,
            case.name,
            created.status,
            created.body
        );
        assert!(
            created.message().contains("64"),
            "case {:?}: the refusal must state the documented rule, got {:?}",
            case.case,
            created.message()
        );
    }
}

/// One row of the enum table: the body, and whether every enum value in it is one the
/// model declares.
struct EnumCase {
    case: &'static str,
    body: Value,
    accepted: bool,
}

/// The rows of the enum table.
///
/// Each value set is the model's own (smithy_query shape="TlsPolicy,HttpsPolicy,
/// SuppressionListReasons,SuppressionListScope,FeatureStatus"), and each member is
/// exercised with a declared value and with one the model does not declare.
fn enum_cases() -> Vec<EnumCase> {
    vec![
        EnumCase {
            case: "TlsPolicy REQUIRE is declared",
            body: json!({
                "ConfigurationSetName": "enum-tls-ok",
                "DeliveryOptions": {"TlsPolicy": "REQUIRE"},
            }),
            accepted: true,
        },
        EnumCase {
            case: "TlsPolicy REQUIRED is not declared",
            body: json!({
                "ConfigurationSetName": "enum-tls-bad",
                "DeliveryOptions": {"TlsPolicy": "REQUIRED"},
            }),
            accepted: false,
        },
        EnumCase {
            case: "HttpsPolicy OPTIONAL is declared",
            body: json!({
                "ConfigurationSetName": "enum-https-ok",
                "TrackingOptions": {
                    "CustomRedirectDomain": "click.example.com",
                    "HttpsPolicy": "OPTIONAL",
                },
            }),
            accepted: true,
        },
        EnumCase {
            case: "HttpsPolicy HTTPS_ONLY is not declared",
            body: json!({
                "ConfigurationSetName": "enum-https-bad",
                "TrackingOptions": {
                    "CustomRedirectDomain": "click.example.com",
                    "HttpsPolicy": "HTTPS_ONLY",
                },
            }),
            accepted: false,
        },
        EnumCase {
            case: "SuppressedReasons BOUNCE is declared",
            body: json!({
                "ConfigurationSetName": "enum-reason-ok",
                "SuppressionOptions": {"SuppressedReasons": ["BOUNCE"]},
            }),
            accepted: true,
        },
        EnumCase {
            case: "SuppressedReasons SPAM is not declared",
            body: json!({
                "ConfigurationSetName": "enum-reason-bad",
                "SuppressionOptions": {"SuppressedReasons": ["BOUNCE", "SPAM"]},
            }),
            accepted: false,
        },
        EnumCase {
            case: "SuppressionScope TENANT is declared",
            body: json!({
                "ConfigurationSetName": "enum-scope-ok",
                "SuppressionOptions": {"SuppressionScope": "TENANT"},
            }),
            accepted: true,
        },
        EnumCase {
            case: "SuppressionScope GLOBAL is not declared",
            body: json!({
                "ConfigurationSetName": "enum-scope-bad",
                "SuppressionOptions": {"SuppressionScope": "GLOBAL"},
            }),
            accepted: false,
        },
        EnumCase {
            case: "EngagementMetrics DISABLED is declared",
            body: json!({
                "ConfigurationSetName": "enum-dashboard-ok",
                "VdmOptions": {"DashboardOptions": {"EngagementMetrics": "DISABLED"}},
            }),
            accepted: true,
        },
        EnumCase {
            case: "EngagementMetrics ON is not declared",
            body: json!({
                "ConfigurationSetName": "enum-dashboard-bad",
                "VdmOptions": {"DashboardOptions": {"EngagementMetrics": "ON"}},
            }),
            accepted: false,
        },
        EnumCase {
            case: "OptimizedSharedDelivery ENABLED is declared",
            body: json!({
                "ConfigurationSetName": "enum-guardian-ok",
                "VdmOptions": {"GuardianOptions": {"OptimizedSharedDelivery": "ENABLED"}},
            }),
            accepted: true,
        },
        EnumCase {
            case: "OptimizedSharedDelivery YES is not declared",
            body: json!({
                "ConfigurationSetName": "enum-guardian-bad",
                "VdmOptions": {"GuardianOptions": {"OptimizedSharedDelivery": "YES"}},
            }),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn an_enum_member_outside_its_modelled_value_set_is_refused() {
    let ses = Ses::new();

    for case in enum_cases() {
        let created = ses.create_set(case.body.clone()).await;
        let name = created_name(&case.body);
        if case.accepted {
            assert_eq!(
                created.status, 200,
                "case {:?}: expected 200, got {} {:?}",
                case.case, created.status, created.body
            );
            let read = ses.get_set(&name).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: expected the accepted set to read back, got {} {:?}",
                case.case, read.status, read.body
            );
            continue;
        }
        assert_eq!(
            (created.status, created.error_code().as_str()),
            (400, "BadRequestException"),
            "case {:?}: expected 400 BadRequestException, got {} {:?}",
            case.case,
            created.status,
            created.body
        );
        // A refused create stores nothing: the name is still free.
        let read = ses.get_set(&name).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (404, "NotFoundException"),
            "case {:?}: a refused create must store nothing, got {} {:?}",
            case.case,
            read.status,
            read.body
        );
    }
}

#[tokio::test]
async fn a_name_the_account_does_not_hold_is_refused_as_not_found() {
    let ses = Ses::new();

    // A set that does exist, so the refusal below is about the name asked for rather than
    // about an empty store.
    let created = ses.create_set(json!({"ConfigurationSetName": "present-set"})).await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateConfigurationSet, got {} {:?}",
        created.status, created.body
    );

    for name in ["absent-set", "Present-Set", "present_set"] {
        let read = ses.get_set(name).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (404, "NotFoundException"),
            "case {name:?}: expected 404 NotFoundException, got {} {:?}",
            read.status,
            read.body
        );
        assert!(
            read.message().contains(name),
            "case {name:?}: the refusal must name the set asked for, got {:?}",
            read.message()
        );
    }
}

/// The `ConfigurationSetName` a table row's request body carries.
fn created_name(body: &Value) -> String {
    body.get("ConfigurationSetName")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
