// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how this service refuses a value outside the form an
//! `IAMRoleArn` member's `@arnReference` names.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a refusal that restates a VALUE CONSTRAINT on a member — one the
//! model states as an `@arnReference` rather than as a `@pattern` the boundary validator
//! could apply — is stated by the layer that states every other constraint refusal of
//! this endpoint, in that layer's words and in its shape: HTTP 400 `ValidationException`
//! whose LOWERCASE `message` member carries
//! `1 validation error detected: Value '<value>' at '<member path>' failed to satisfy
//! constraint: Member must satisfy regular expression pattern: <expression>`, with the
//! member named by its validation path, the value quoted as the request sent it, and the
//! expression quoted WITHOUT the `^`/`$` anchor pair a whole-value expression is wrapped
//! in. The expression the message advertises is the one the service enforces, which each
//! row checks by running the published constant through the same regular-expression
//! engine the boundary validator uses.
//!
//! The rows span what the rule generalises over: which operation the request reaches and
//! so which of the three members the value travels on (a body member of one operation, a
//! member nested inside a list element, a body member of another), the position of the
//! list element, and which part of the ARN form the value breaks. The accepted rows span
//! what the form leaves free — partition, account, role path and name — and are read back
//! from the store, so a refusal is told apart from a value the service simply dropped.

use std::sync::Arc;

use regex::Regex;
use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::IAM_ROLE_ARN_PATTERN;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The configuration type every definition below deploys. No case here is about the
/// type, the version or the parameters.
const TYPE: &str = "AWSQuickSetupType-SSMHostMgmt";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`].
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call, with the `x-amzn-errortype` header folded into
/// the body under `__type` so one accessor reads either.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment: a non-greedy label
/// may not carry a raw `/`.
fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

/// One definition of [`TYPE`] carrying every parameter that type's documented table
/// requires, deployed through an organizational unit — so it is not a LOCAL deployment
/// and no rule about a local deployment's roles takes part in a case here.
fn definition(role_arn: Option<&str>) -> Value {
    let mut definition = json!({
        "Type": TYPE,
        "Parameters": super::type_parameter_fixtures::with_required_parameters(
            TYPE,
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": REGION }),
        ),
    });
    if let Some(role_arn) = role_arn {
        definition["LocalDeploymentAdministrationRoleArn"] = json!(role_arn);
    }
    definition
}

/// `POST /configurationManager` with the definitions a case supplies.
async fn create(router: &MultiServiceRouter, definitions: Vec<Value>) -> (u16, Value) {
    call(
        router,
        "POST",
        "/configurationManager",
        Some(&json!({ "Name": "role-arns", "ConfigurationDefinitions": definitions })),
    )
    .await
}

/// A stored manager and the id of its one definition, for the update rows.
async fn stored_definition(router: &MultiServiceRouter) -> (String, String) {
    let (status, answer) = create(router, vec![definition(None)]).await;
    assert_eq!(
        status, 200,
        "CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    let manager_arn = answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ManagerArn, got {answer:?}"))
        .to_string();
    let (status, read) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(&manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "GetConfigurationManager must succeed, got {status} / {read:?}"
    );
    let id = read
        .get("ConfigurationDefinitions")
        .and_then(Value::as_array)
        .and_then(|definitions| definitions.first())
        .and_then(|definition| definition.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected one stored definition, got {read:?}"))
        .to_string();
    (manager_arn, id)
}

/// Which member of which operation a row sends its value on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Member {
    /// `UpdateServiceSettings$ExplorerEnablingRoleArn`.
    ExplorerEnabling,
    /// `CreateConfigurationManager`'s `ConfigurationDefinitions[n].\
    /// LocalDeploymentAdministrationRoleArn`, with the value on the element at the
    /// 1-based position the variant carries.
    CreatedDefinition { position: usize },
    /// `UpdateConfigurationDefinition$LocalDeploymentAdministrationRoleArn`.
    UpdatedDefinition,
}

impl Member {
    /// The validation path a clause names this member by.
    fn path(self) -> String {
        match self {
            Self::ExplorerEnabling => "explorerEnablingRoleArn".to_string(),
            Self::CreatedDefinition { position } => format!(
                "configurationDefinitions.{position}.member.localDeploymentAdministrationRoleArn"
            ),
            Self::UpdatedDefinition => {
                "localDeploymentAdministrationRoleArn".to_string()
            }
        }
    }

    /// Send `value` on this member.
    async fn send(self, router: &MultiServiceRouter, value: &str) -> (u16, Value) {
        match self {
            Self::ExplorerEnabling => {
                call(
                    router,
                    "PUT",
                    "/serviceSettings",
                    Some(&json!({ "ExplorerEnablingRoleArn": value })),
                )
                .await
            }
            Self::CreatedDefinition { position } => {
                // Every element before the one under test carries no role at all, so the
                // clause names the position of the element the value travelled on.
                let mut definitions: Vec<Value> =
                    (1..position).map(|_| definition(None)).collect();
                definitions.push(definition(Some(value)));
                create(router, definitions).await
            }
            Self::UpdatedDefinition => {
                let (manager_arn, id) = stored_definition(router).await;
                call(
                    router,
                    "PUT",
                    &format!(
                        "/configurationDefinition/{}/{}",
                        label(&manager_arn),
                        label(&id)
                    ),
                    Some(&json!({ "LocalDeploymentAdministrationRoleArn": value })),
                )
                .await
            }
        }
    }
}

/// The expression the refusals advertise: the published form less the one anchor pair
/// wrapping it, which is how AWS quotes a whole-value expression.
fn advertised_expression() -> &'static str {
    IAM_ROLE_ARN_PATTERN
        .strip_prefix('^')
        .and_then(|inner| inner.strip_suffix('$'))
        .expect("the published form is written with both anchors")
}

/// Values no `IAMRoleArn` member may carry, each breaking one part of the form.
const REFUSED: &[(&str, &str)] = &[
    ("empty", ""),
    ("not an ARN at all", "not-a-role-arn"),
    ("an ARN prefix only", "arn:aws:iam"),
    ("an IAM user, not a role", "arn:aws:iam::000000000000:user/bob"),
    ("no resource after the role type", "arn:aws:iam::000000000000:role"),
    ("another service", "arn:aws:ssm:us-west-2:000000000000:role/Explorer"),
    ("an empty partition", "arn::iam::000000000000:role/Explorer"),
    (
        "a partition the ARN formats do not publish",
        "arn:aws-moon:iam::000000000000:role/Explorer",
    ),
    (
        // An IAM ARN carries no Region of its own.
        "a Region in the IAM field",
        "arn:aws:iam:us-west-2:000000000000:role/Explorer",
    ),
    (
        "an account that is not twelve digits",
        "arn:aws:iam::12345:role/Explorer",
    ),
    (
        "an account that is not digits at all",
        "arn:aws:iam::aws-account:role/Explorer",
    ),
];

/// Values every `IAMRoleArn` member must take, spanning what the form leaves free.
const ACCEPTED: &[(&str, &str)] = &[
    ("a plain role", "arn:aws:iam::000000000000:role/QuickSetupAdministration"),
    (
        "a role reached through a path",
        "arn:aws:iam::123456789012:role/quicksetup/explorer/Role",
    ),
    (
        "a service-linked role",
        "arn:aws:iam::123456789012:role/aws-service-role/ssm.amazonaws.com/AWSServiceRoleForAmazonSSM",
    ),
    ("the China partition", "arn:aws-cn:iam::123456789012:role/Explorer"),
    ("a GovCloud partition", "arn:aws-us-gov:iam::123456789012:role/Explorer"),
    ("an ISO partition", "arn:aws-iso-b:iam::123456789012:role/Explorer"),
    // The two rows below are where the published expression is deliberately PERMISSIVE,
    // and they are here so that permissiveness is pinned rather than merely unrefused:
    // the `/` of the role resource is inside the class the name is drawn from, and the
    // expression ends in an unconstrained run, so a resource that stops at the separator
    // and one carrying further `:`-separated text both satisfy the form.
    ("a role resource with no name after it", "arn:aws:iam::000000000000:role/"),
    (
        "text after the role name",
        "arn:aws:iam::000000000000:role/Explorer:extra:more",
    ),
];

/// Every member that takes an `IAMRoleArn`, and every value outside the published form,
/// earns the boundary layer's own envelope: the counted sentence in the lowercase
/// `message` member, naming the member's validation path, the value sent and the
/// expression without its anchors.
#[tokio::test]
async fn a_value_outside_the_published_role_arn_form_is_refused_in_the_boundary_envelope() {
    let form = Regex::new(IAM_ROLE_ARN_PATTERN).expect("the published form compiles");
    let members = [
        Member::ExplorerEnabling,
        Member::CreatedDefinition { position: 1 },
        // A second element, so the clause's position is the element the value was on.
        Member::CreatedDefinition { position: 2 },
        Member::UpdatedDefinition,
    ];

    for member in members {
        for (value_case, value) in REFUSED {
            let case = format!("{value_case} on {member:?}");
            // The enforced form really does refuse this value, judged by the engine the
            // boundary validator runs a modelled expression through.
            assert!(
                !form.is_match(value),
                "case {case:?}: the published form must refuse {value:?}"
            );
            // One emulator per row, so no row observes another row's writes.
            let router = emulator();
            let (status, answer) = member.send(&router, value).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the modelled 400, got {status} / {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: expected ValidationException, got {code:?} in {answer:?}"
            );
            let expected = format!(
                "1 validation error detected: Value '{value}' at '{}' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {}",
                member.path(),
                advertised_expression()
            );
            let actual = answer
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the lowercase message member, got {answer:?}")
                });
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(IAM_ROLE_ARN_PATTERN),
                "case {case:?}: the anchored expression must not be quoted, got {actual:?}"
            );
        }
    }
}

/// The other edge of the same rule: a value the published form admits is taken by every
/// member that carries one, and is read back from the store exactly as it was written.
#[tokio::test]
async fn a_value_the_published_form_admits_is_stored_on_every_member_that_takes_one() {
    let form = Regex::new(IAM_ROLE_ARN_PATTERN).expect("the published form compiles");

    for (value_case, value) in ACCEPTED {
        assert!(
            form.is_match(value),
            "case {value_case:?}: the published form must admit {value:?}"
        );

        // `UpdateServiceSettings`, read back through `GetServiceSettings`.
        let router = emulator();
        let (status, answer) = Member::ExplorerEnabling.send(&router, value).await;
        assert_eq!(
            status, 200,
            "case {value_case:?}: the update must succeed, got {status} / {answer:?}"
        );
        let (status, read) = call(&router, "GET", "/serviceSettings", None).await;
        assert_eq!(
            status, 200,
            "case {value_case:?}: GetServiceSettings must succeed, got {status} / {read:?}"
        );
        let reported = read
            .get("ServiceSettings")
            .and_then(|settings| settings.get("ExplorerEnablingRoleArn"))
            .and_then(Value::as_str);
        assert_eq!(
            reported,
            Some(*value),
            "case {value_case:?}: expected the role the update wrote, got {read:?}"
        );

        // `CreateConfigurationManager`, read back through `GetConfigurationManager`.
        let router = emulator();
        let (status, answer) = create(&router, vec![definition(Some(value))]).await;
        assert_eq!(
            status, 200,
            "case {value_case:?}: the create must succeed, got {status} / {answer:?}"
        );
        let manager_arn = answer
            .get("ManagerArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {value_case:?}: expected a ManagerArn, got {answer:?}"))
            .to_string();
        let (status, read) = call(
            &router,
            "GET",
            &format!("/configurationManager/{}", label(&manager_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {value_case:?}: GetConfigurationManager must succeed, got {status} / {read:?}"
        );
        let stored = read
            .get("ConfigurationDefinitions")
            .and_then(Value::as_array)
            .and_then(|definitions| definitions.first())
            .cloned()
            .unwrap_or_else(|| panic!("case {value_case:?}: expected one definition, got {read:?}"));
        assert_eq!(
            stored
                .get("LocalDeploymentAdministrationRoleArn")
                .and_then(Value::as_str),
            Some(*value),
            "case {value_case:?}: expected the role the create wrote, got {stored:?}"
        );

        // `UpdateConfigurationDefinition`, read back the same way: its own manager, so
        // the value this row writes is the only one the definition has ever carried.
        let router = emulator();
        let (manager_arn, id) = stored_definition(&router).await;
        let (status, answer) = call(
            &router,
            "PUT",
            &format!(
                "/configurationDefinition/{}/{}",
                label(&manager_arn),
                label(&id)
            ),
            Some(&json!({ "LocalDeploymentAdministrationRoleArn": value })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {value_case:?}: the definition update must succeed, got {status} / {answer:?}"
        );
        let (status, read) = call(
            &router,
            "GET",
            &format!("/configurationManager/{}", label(&manager_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {value_case:?}: GetConfigurationManager must succeed, got {status} / {read:?}"
        );
        let updated = read
            .get("ConfigurationDefinitions")
            .and_then(Value::as_array)
            .and_then(|definitions| definitions.first())
            .cloned()
            .unwrap_or_else(|| panic!("case {value_case:?}: expected one definition, got {read:?}"));
        assert_eq!(
            updated
                .get("LocalDeploymentAdministrationRoleArn")
                .and_then(Value::as_str),
            Some(*value),
            "case {value_case:?}: expected the role the update wrote, got {updated:?}"
        );
    }
}
