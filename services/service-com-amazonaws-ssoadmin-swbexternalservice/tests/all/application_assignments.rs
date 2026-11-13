// SPDX-License-Identifier: CC0-1.0

//! The application / application-assignment slice end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. `CreateApplication` PERSISTS the request under a freshly minted `ApplicationArn`
//!    of the form the model's `ApplicationArn` pattern requires, and
//!    `DescribeApplication` REPORTS member for member what the create stored — for every
//!    combination of `Name`, `Description`, `Status`, `PortalOptions` and `Tags` the
//!    model admits — with `ApplicationAccount` the account of its own ARN and
//!    `CreatedFrom` the Region the create ran in;
//! 2. `ClientToken` is `CreateApplication`'s `@idempotencyToken`: the same token with the
//!    same parameters answers with the ARN the first call minted and creates nothing, and
//!    the same token with any different parameter is refused `ConflictException`;
//! 3. `CreateApplicationAssignment` grants ONE direct assignment per (application,
//!    principal type, principal id) tuple — a grant of one the application already holds
//!    CONVERGES on it, answering the modelled empty 200 and storing no second row — and
//!    `DescribeApplicationAssignment` reports exactly that tuple;
//! 4. an `ApplicationArn` no create minted is refused `ResourceNotFoundException` by
//!    every operation of the slice, and a principal that holds no DIRECT assignment to an
//!    application is refused the same way, never answered with a 200;
//! 5. a describe is a REPORT: repeating it answers identically and changes nothing a
//!    later caller sees.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, call_in_region, create_application_assignment as create_assignment,
    describe_application_assignment as describe_assignment, error_code, hold_deletion, router,
    router_over, seeded_instance_arn, with_store, TEST_REGION,
};

/// The AWS-owned provider a customer managed OAuth 2.0 application is created under. Its
/// id is an entry of the catalog this service publishes
/// (`state::BUILT_IN_APPLICATION_PROVIDERS`), reachable over the wire through
/// `ListApplicationProviders`.
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// One application to create: the parameters, as a row of the table.
///
/// Shared with the other suites of this slice, so an application a rule is asserted over
/// is built one way (`application_assignment_describe.rs` varies `Status` over it).
#[derive(Debug, Clone)]
pub(crate) struct Application {
    label: String,
    provider_arn: String,
    name: String,
    description: Option<String>,
    status: Option<String>,
    portal_options: Option<Value>,
    tags: Vec<(String, String)>,
    client_token: Option<String>,
}

impl Application {
    pub(crate) fn new(label: &str, name: &str) -> Self {
        Self {
            label: label.to_string(),
            provider_arn: CUSTOM_PROVIDER_ARN.to_string(),
            name: name.to_string(),
            description: None,
            status: None,
            portal_options: None,
            tags: Vec::new(),
            client_token: None,
        }
    }

    fn with_provider(mut self, provider_arn: &str) -> Self {
        self.provider_arn = provider_arn.to_string();
        self
    }

    fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    pub(crate) fn with_status(mut self, status: &str) -> Self {
        self.status = Some(status.to_string());
        self
    }

    fn with_portal_options(mut self, portal_options: Value) -> Self {
        self.portal_options = Some(portal_options);
        self
    }

    fn with_tags(mut self, tags: &[(&str, &str)]) -> Self {
        self.tags = tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        self
    }

    fn with_client_token(mut self, token: &str) -> Self {
        self.client_token = Some(token.to_string());
        self
    }

    /// The `CreateApplication` request body for this row.
    fn request(&self, instance_arn: &str) -> Value {
        let mut body = json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": self.provider_arn,
            "Name": self.name,
        });
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(status) = &self.status {
            body["Status"] = json!(status);
        }
        if let Some(portal_options) = &self.portal_options {
            body["PortalOptions"] = portal_options.clone();
        }
        if !self.tags.is_empty() {
            body["Tags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            );
        }
        if let Some(token) = &self.client_token {
            body["ClientToken"] = json!(token);
        }
        body
    }

    /// The `Status` a describe must report: the one the request named, or the state an
    /// application nobody disabled is in.
    fn expected_status(&self) -> String {
        self.status
            .clone()
            .unwrap_or_else(|| "ENABLED".to_string())
    }

    /// The `PortalOptions` a describe must report: the caller's own, with `Visibility`
    /// filled in from the model's `@default("ENABLED")` on `ApplicationVisibility`
    /// (`smithy_query(query="shape",shape="ApplicationVisibility")`) wherever the request
    /// left it out.
    fn expected_portal_options(&self) -> Value {
        let mut options = self
            .portal_options
            .clone()
            .unwrap_or_else(|| json!({}));
        if options.get("Visibility").is_none() {
            options["Visibility"] = json!("ENABLED");
        }
        options
    }
}

/// The rows differ along what rule 1 generalises over: the length and character class of
/// `Name` (`len 1..100`, `^[\S\s]*$`, so whitespace and punctuation are in), whether
/// `Description` is sent (`len 1..128`), both values of `ApplicationStatus`
/// (`ENABLED | DISABLED`), the `PortalOptions` forms the model admits (absent, visibility
/// only, each `SignInOrigin`), the number of `Tags` and whether a `ClientToken` is sent.
/// All constraints are from
/// `smithy_query(query="operation",operation="CreateApplication")` and
/// `smithy_query(query="shape",shape="PortalOptions,SignInOptions,ApplicationStatus")`.
fn applications() -> Vec<Application> {
    vec![
        Application::new("the shortest Name the model admits, nothing else sent", "a"),
        Application::new(
            "Name at the 100-character limit, with the whitespace ^[\\S\\s]*$ admits",
            &format!("Billing app {}", "x".repeat(88)),
        )
        .with_description("The finance team's billing portal")
        .with_status("ENABLED")
        .with_portal_options(json!({"Visibility": "ENABLED"}))
        .with_tags(&[("owner", "platform"), ("Env", "staging"), ("cost-centre", "42")])
        .with_client_token("apl-token-1"),
        Application::new("a DISABLED application with no portal options", "disabled-app")
            .with_status("DISABLED")
            .with_description("d")
            .with_tags(&[("single", "tag")]),
        Application::new(
            "SignInOrigin APPLICATION, which requires an ApplicationUrl",
            "redirecting-app",
        )
        .with_portal_options(json!({
            "SignInOptions": {
                "Origin": "APPLICATION",
                "ApplicationUrl": "https://app.example.com/sso/login?tenant=one",
            },
            "Visibility": "DISABLED",
        }))
        .with_client_token("apl-token-2"),
        Application::new(
            "SignInOrigin IDENTITY_CENTER and no Visibility, which takes the model default",
            "idp-initiated-app",
        )
        .with_portal_options(json!({"SignInOptions": {"Origin": "IDENTITY_CENTER"}}))
        .with_description("Description at the 128-character limit padded out here")
        .with_tags(&[("only", "one")]),
    ]
}

/// The principals rule 3 is asserted over: both values of `PrincipalType`
/// (`USER | GROUP`) and both forms the model's `PrincipalId` pattern admits — a bare GUID
/// in lower and upper hex, and the `[0-9a-f]{10}-` prefixed form at the 47-character
/// length limit (`smithy_query(query="operation",operation="CreateApplicationAssignment")`).
const PRINCIPALS: &[(&str, &str, &str)] = &[
    (
        "a lowercase-hex USER GUID",
        "USER",
        "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
    ),
    (
        "an uppercase-hex GROUP GUID",
        "GROUP",
        "A1B2C3D4-E5F6-4071-8293-A4B5C6D7E8F9",
    ),
    (
        "a prefixed USER id at the 47-character limit",
        "USER",
        "9067f3a1c2-01234567-89ab-4cde-8f01-23456789abcd",
    ),
    (
        "a prefixed GROUP id",
        "GROUP",
        "0a1b2c3d4e-fedcba98-7654-4321-8fed-cba987654321",
    ),
];

/// Whether `arn` has the shape the model's `ApplicationArn` pattern requires for an
/// application of `instance_arn`:
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`.
fn is_application_arn(arn: &str, instance_arn: &str) -> bool {
    let instance_id = instance_arn.rsplit('/').next().unwrap_or_default();
    let Some(rest) = arn.strip_prefix("arn:aws:sso::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some(suffix) = resource.strip_prefix(&format!("application/{instance_id}/apl-")) else {
        return false;
    };
    suffix.len() == 16 && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The account segment of an `ApplicationArn`.
fn arn_account(arn: &str) -> String {
    arn.split(':').nth(4).unwrap_or_default().to_string()
}

async fn create_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    application: &Application,
) -> (u16, Value) {
    call(
        router,
        "CreateApplication",
        &application.request(instance_arn),
    )
    .await
}

async fn describe_application(router: &MultiServiceRouter, application_arn: &str) -> (u16, Value) {
    call(
        router,
        "DescribeApplication",
        &json!({"ApplicationArn": application_arn}),
    )
    .await
}

/// Create one application and return the ARN it minted, failing the test if it did not.
pub(crate) async fn minted_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    application: &Application,
) -> String {
    let (status, body) = create_application(router, instance_arn, application).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateApplication must answer 200, got {status} {body}",
        application.label
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateApplication minted no ApplicationArn: {body}"))
        .to_string()
}

/// Rule 1 (and rule 5): every row round-trips through the store, under an ARN of the
/// modelled form that no other row shares.
#[tokio::test]
async fn every_created_application_reads_back_exactly_what_was_stored() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let mut minted: Vec<String> = Vec::new();

    for application in applications() {
        let case = &application.label;
        let (status, body) = create_application(&router, &instance_arn, &application).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create must answer 200, got {status} {body}"
        );
        let arn = body["ApplicationArn"].as_str().unwrap_or_default().to_string();
        assert!(
            is_application_arn(&arn, &instance_arn),
            "case {case:?}: minted ARN is not a modelled ApplicationArn of this instance, \
             got {arn:?}"
        );
        assert!(
            !minted.contains(&arn),
            "case {case:?}: each create mints a fresh ARN, got the already-minted {arn:?}"
        );
        assert_eq!(
            body["InstanceArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {case:?}: create must report the owning instance, got {body}"
        );
        let identity_store_arn = body["IdentityStoreArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            identity_store_arn.starts_with("arn:aws:identitystore:"),
            "case {case:?}: create must report the instance's identity store, got {body}"
        );
        minted.push(arn.clone());

        let (status, described) = describe_application(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe of the created application must answer 200, got \
             {status} {described}"
        );
        assert_eq!(
            described["ApplicationArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: describe must report the ARN it was asked about, got {described}"
        );
        assert_eq!(
            described["ApplicationProviderArn"].as_str(),
            Some(application.provider_arn.as_str()),
            "case {case:?}: describe must report the stored provider, got {described}"
        );
        assert_eq!(
            described["Name"].as_str(),
            Some(application.name.as_str()),
            "case {case:?}: describe must report the stored Name, got {described}"
        );
        assert_eq!(
            described["Description"].as_str(),
            application.description.as_deref(),
            "case {case:?}: describe must report the stored Description, got {described}"
        );
        assert_eq!(
            described["Status"].as_str(),
            Some(application.expected_status().as_str()),
            "case {case:?}: describe must report the stored Status, got {described}"
        );
        assert_eq!(
            described["PortalOptions"],
            application.expected_portal_options(),
            "case {case:?}: describe must report the stored PortalOptions, got {described}"
        );
        assert_eq!(
            described["InstanceArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {case:?}: describe must report the owning instance, got {described}"
        );
        assert_eq!(
            described["IdentityStoreArn"].as_str(),
            Some(identity_store_arn.as_str()),
            "case {case:?}: describe must report the same identity store the create did, \
             got {described}"
        );
        assert_eq!(
            described["ApplicationAccount"].as_str(),
            Some(arn_account(&arn).as_str()),
            "case {case:?}: ApplicationAccount must be the account of the application's \
             own ARN, got {described}"
        );
        assert_eq!(
            described["CreatedFrom"].as_str(),
            Some(TEST_REGION),
            "case {case:?}: CreatedFrom must be the Region the create ran in, got {described}"
        );
        assert!(
            described["CreatedDate"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: CreatedDate must be the instant of the create, got {described}"
        );

        // Rule 5: the describe reported rather than mutated, so a second one answers
        // identically.
        let (status, again) = describe_application(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated describe must answer 200, got {status} {again}"
        );
        assert_eq!(
            again, described,
            "case {case:?}: a describe must change nothing a later caller sees, got {again}"
        );
    }

    // Every earlier application is still readable after the later creates: a create adds
    // a record, it does not replace the store.
    for (application, arn) in applications().iter().zip(&minted) {
        let (status, body) = describe_application(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: an application created earlier must still describe, got {status} \
             {body}",
            application.label
        );
    }
}

/// Rule 1, the `CreatedFrom` half: the Region comes from the REQUEST, not from a constant
/// the emulator was built with.
#[tokio::test]
async fn created_from_is_the_region_the_create_ran_in() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // Regions inside `RegionName`'s `^([a-z]+-){2,3}\d$` pattern, including a
    // three-segment one.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-3", "us-gov-west-1"] {
        let application = Application::new("one application per Region", "regional-app");
        let (status, body) = call_in_region(
            &router,
            region,
            "CreateApplication",
            &application.request(&instance_arn),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: create must answer 200, got {status} {body}"
        );
        let arn = body["ApplicationArn"].as_str().unwrap_or_default().to_string();
        let (status, described) = describe_application(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {region:?}: describe must answer 200, got {status} {described}"
        );
        assert_eq!(
            described["CreatedFrom"].as_str(),
            Some(region),
            "case {region:?}: CreatedFrom must be the Region of the create, got {described}"
        );
    }
}

/// Rule 2: a replayed `ClientToken` with the same parameters answers with the first
/// call's ARN, and the same token with a different parameter is a conflict.
#[tokio::test]
async fn a_replayed_client_token_mints_no_second_application_and_a_changed_parameter_conflicts() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    for application in applications()
        .into_iter()
        .filter(|row| row.client_token.is_some())
    {
        let case = application.label.clone();
        let first = minted_application(&router, &instance_arn, &application).await;
        let stored_after_first = with_store(&engine, |store| store.applications.len());

        // The identical request under the identical token: a retry, not a second
        // application.
        let (status, replayed) = create_application(&router, &instance_arn, &application).await;
        assert_eq!(
            status, 200,
            "case {case:?}: replaying the token must answer 200, got {status} {replayed}"
        );
        assert_eq!(
            replayed["ApplicationArn"].as_str(),
            Some(first.as_str()),
            "case {case:?}: a replay must answer with the first ARN {first:?}, got {replayed}"
        );
        assert_eq!(
            with_store(&engine, |store| store.applications.len()),
            stored_after_first,
            "case {case:?}: a replay must store no second application"
        );

        // Tag ORDER is not a parameter difference: a tag set carries no order.
        if application.tags.len() > 1 {
            let mut reordered = application.clone();
            reordered.tags.reverse();
            let (status, body) = create_application(&router, &instance_arn, &reordered).await;
            assert_eq!(
                body["ApplicationArn"].as_str(),
                Some(first.as_str()),
                "case {case:?}: reordering Tags is the same request, expected {first:?}, \
                 got {status} {body}"
            );
        }

        // One mutated member per row: each is a parameter the token is scoped to, so each
        // is a conflict rather than a replay.
        let mut mutations: Vec<(&str, Application)> = Vec::new();
        let mut renamed = application.clone();
        renamed.name = format!("{}-other", &application.name[..10]);
        mutations.push(("Name", renamed));
        let mut re_described = application.clone();
        re_described.description = Some("another description".to_string());
        mutations.push(("Description", re_described));
        let mut re_statused = application.clone();
        re_statused.status = Some(if application.expected_status() == "ENABLED" {
            "DISABLED".to_string()
        } else {
            "ENABLED".to_string()
        });
        mutations.push(("Status", re_statused));
        let mut re_portaled = application.clone();
        re_portaled.portal_options = Some(json!({"Visibility": "DISABLED", "SignInOptions": {
            "Origin": "APPLICATION",
            "ApplicationUrl": "https://moved.example.com/sso",
        }}));
        mutations.push(("PortalOptions", re_portaled));
        let mut re_tagged = application.clone();
        re_tagged
            .tags
            .push(("extra".to_string(), "tag".to_string()));
        mutations.push(("Tags", re_tagged));
        // `ApplicationProviderArn` is not among the mutations: the one provider this API
        // creates an application under is the custom one, so a row that changed it would
        // be refused for the provider before the token is ever looked at — the provider
        // rule has its own suite (`application_create_providers.rs`).

        for (member, mutated) in mutations {
            if mutated.provider_arn == application.provider_arn
                && mutated.name == application.name
                && mutated.description == application.description
                && mutated.expected_status() == application.expected_status()
                && mutated.expected_portal_options() == application.expected_portal_options()
                && mutated.tags == application.tags
            {
                continue;
            }
            let (status, body) = create_application(&router, &instance_arn, &mutated).await;
            assert_eq!(
                status, 400,
                "case {case:?} changed {member}: the reused token must be refused 400, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case:?} changed {member}: expected ConflictException, got {body}"
            );
        }

        // The conflicts changed nothing: the first application still reads back.
        let (status, described) = describe_application(&router, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first application must survive the refusals, got {status} \
             {described}"
        );
        assert_eq!(
            described["Name"].as_str(),
            Some(application.name.as_str()),
            "case {case:?}: a refused create must not have altered the stored Name, got \
             {described}"
        );
    }
}

/// Rule 1, the refusal half: a request the model cannot refuse itself — a `SignInOptions`
/// whose `Origin` is `APPLICATION` and that carries no `ApplicationUrl`, which
/// `ApplicationUrl`'s own documentation makes a required parameter — is refused 400
/// `ValidationException` and stores nothing. A provider ARN naming no published provider
/// is a not-found.
#[tokio::test]
async fn a_create_the_model_cannot_refuse_itself_is_refused_rather_than_stored() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "Origin APPLICATION with no ApplicationUrl at all",
            json!({"SignInOptions": {"Origin": "APPLICATION"}, "Visibility": "ENABLED"}),
            "ValidationException",
        ),
        (
            "Origin APPLICATION with no ApplicationUrl and no Visibility",
            json!({"SignInOptions": {"Origin": "APPLICATION"}}),
            "ValidationException",
        ),
    ];

    for (case, portal_options, expected) in cases {
        let application = Application::new(case, "refused-app").with_portal_options(portal_options);
        let (status, body) = create_application(&router, &instance_arn, &application).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the create must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        assert!(
            body["ApplicationArn"].is_null(),
            "case {case:?}: a refusal mints no ARN, got {body}"
        );
    }

    // `Origin` `IDENTITY_CENTER` needs no `ApplicationUrl`: the rule is about the one
    // origin that redirects to the application's own URL.
    let accepted = Application::new(
        "Origin IDENTITY_CENTER with no ApplicationUrl",
        "accepted-app",
    )
    .with_portal_options(json!({"SignInOptions": {"Origin": "IDENTITY_CENTER"}}));
    let (status, body) = create_application(&router, &instance_arn, &accepted).await;
    assert_eq!(
        status, 200,
        "IDENTITY_CENTER needs no ApplicationUrl, got {status} {body}"
    );

    // An `ApplicationProviderArn` the catalog does not publish resolves to nothing.
    let unknown_provider = Application::new("a provider ARN nothing publishes", "no-provider-app")
        .with_provider("arn:aws:sso::aws:applicationProvider/not-a-published-provider");
    let (status, body) = create_application(&router, &instance_arn, &unknown_provider).await;
    assert_eq!(
        status, 400,
        "an unpublished provider must be refused 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "an unpublished provider must be a not-found, got {body}"
    );

    // Exactly one application was stored: the accepted one.
    assert_eq!(
        with_store(&engine, |store| store.applications.len()),
        1,
        "every refused create must have stored nothing"
    );
}

/// Rule 3 (and rule 5): every principal of the table is granted exactly one assignment
/// per application, reads back as the tuple it was granted with, and a repeated grant
/// adds no second row.
#[tokio::test]
async fn every_granted_assignment_reads_back_and_granting_it_twice_grants_it_once() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    // Two applications, so an assignment is pinned to the application it names and not
    // just to the principal.
    let first = minted_application(
        &router,
        &instance_arn,
        &Application::new("the first application", "assignable-one"),
    )
    .await;
    let second = minted_application(
        &router,
        &instance_arn,
        &Application::new("the second application", "assignable-two"),
    )
    .await;

    let mut expected_rows = 0usize;
    for application_arn in [&first, &second] {
        for (label, principal_type, principal_id) in PRINCIPALS {
            let case = format!("{label} on {application_arn}");

            // Before the grant there is no assignment to report.
            let (status, body) =
                describe_assignment(&router, application_arn, principal_type, principal_id).await;
            assert_eq!(
                status, 400,
                "case {case:?}: an ungranted principal must be refused 400, got {status} \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );

            let (status, body) =
                create_assignment(&router, application_arn, principal_type, principal_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the grant must answer 200, got {status} {body}"
            );
            expected_rows += 1;

            let (status, described) =
                describe_assignment(&router, application_arn, principal_type, principal_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the granted assignment must describe, got {status} \
                 {described}"
            );
            assert_eq!(
                described["ApplicationArn"].as_str(),
                Some(application_arn.as_str()),
                "case {case:?}: describe must report the application it was asked about, \
                 got {described}"
            );
            assert_eq!(
                described["PrincipalType"].as_str(),
                Some(*principal_type),
                "case {case:?}: describe must report the granted PrincipalType, got \
                 {described}"
            );
            assert_eq!(
                described["PrincipalId"].as_str(),
                Some(*principal_id),
                "case {case:?}: describe must report the granted PrincipalId, got \
                 {described}"
            );

            // Rule 3: the same grant again converges on the assignment that write made —
            // served, and no second row.
            let (status, body) =
                create_assignment(&router, application_arn, principal_type, principal_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a repeated grant must answer 200, got {status} {body}"
            );
            assert_eq!(
                with_store(&engine, |store| store.application_assignments.len()),
                expected_rows,
                "case {case:?}: a repeated grant must store no second row"
            );

            // Rule 5: the describe is a report, so it answers identically afterwards.
            let (status, again) =
                describe_assignment(&router, application_arn, principal_type, principal_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a repeated describe must answer 200, got {status} {again}"
            );
            assert_eq!(
                again, described,
                "case {case:?}: a describe must change nothing a later caller sees, got \
                 {again}"
            );
        }
    }

    // Every grant of the table is still readable at the end: a grant adds a row, it does
    // not replace the store.
    for application_arn in [&first, &second] {
        for (label, principal_type, principal_id) in PRINCIPALS {
            let (status, body) =
                describe_assignment(&router, application_arn, principal_type, principal_id).await;
            assert_eq!(
                status, 200,
                "case {label:?} on {application_arn}: an earlier grant must still \
                 describe, got {status} {body}"
            );
        }
    }
}

/// Rule 3, the identity half: the assignment is the (application, principal type,
/// principal id) tuple, so a grant for one tuple is not an assignment for a tuple that
/// differs in ANY part.
#[tokio::test]
async fn an_assignment_is_only_reported_for_the_tuple_it_was_granted_with() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let granted_app = minted_application(
        &router,
        &instance_arn,
        &Application::new("the application the grant names", "granted-app"),
    )
    .await;
    let other_app = minted_application(
        &router,
        &instance_arn,
        &Application::new("another application", "other-app"),
    )
    .await;

    let principal_id = "f81d4fae-7dec-11d0-a765-00a0c91e6bf6";
    let (status, body) = create_assignment(&router, &granted_app, "USER", principal_id).await;
    assert_eq!(status, 200, "the grant must answer 200, got {status} {body}");

    // Each row differs from the granted tuple in exactly one part.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "the same principal on another application",
            other_app.as_str(),
            "USER",
            principal_id,
        ),
        (
            "the same id under the other PrincipalType",
            granted_app.as_str(),
            "GROUP",
            principal_id,
        ),
        (
            "another principal id on the granted application",
            granted_app.as_str(),
            "USER",
            "00000000-0000-4000-8000-000000000000",
        ),
    ];

    for (case, application_arn, principal_type, id) in cases {
        let (status, body) =
            describe_assignment(&router, application_arn, principal_type, id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a tuple nothing granted must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // The granted tuple still reads back after every refusal.
    let (status, body) = describe_assignment(&router, &granted_app, "USER", principal_id).await;
    assert_eq!(
        status, 200,
        "the granted assignment must survive the refusals, got {status} {body}"
    );
}

/// Rule 4: an `ApplicationArn` no create minted is a not-found for every operation of the
/// slice, never a 200 and never a stored grant.
#[tokio::test]
async fn an_application_arn_no_create_minted_is_refused_by_every_operation() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    // One application exists, so each refusal is about the ARN asked for and not about an
    // empty store.
    let stored = minted_application(
        &router,
        &instance_arn,
        &Application::new("the one stored application", "stored-app"),
    )
    .await;
    let account = arn_account(&stored);
    let apl_id = stored.rsplit('/').next().unwrap_or_default().to_string();
    let application_arn = |partition: &str, account: &str, instance: &str, apl: &str| {
        format!("arn:{partition}:sso::{account}:application/{instance}/{apl}")
    };

    // The rows span the parts of the ARN that identify the application: the `apl-`
    // segment, the instance segment, the account segment and the partition.
    let cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid apl- id that was never minted",
            application_arn(
                "aws",
                &account,
                &instance_id,
                "apl-0000000000000000",
            ),
        ),
        (
            "the stored application's id under another instance",
            application_arn("aws", &account, "ssoins-0000000000000000", &apl_id),
        ),
        (
            "the stored application under another account",
            application_arn("aws", "210987654321", &instance_id, &apl_id),
        ),
        (
            "the stored application in another partition",
            application_arn("aws-us-gov", &account, &instance_id, &apl_id),
        ),
    ];

    for (case, arn) in cases {
        assert_ne!(
            arn, stored,
            "case {case:?}: the row must name a DIFFERENT ARN than the stored one"
        );
        for operation in ["DescribeApplication", "CreateApplicationAssignment"] {
            let (status, body) = if operation == "DescribeApplication" {
                describe_application(&router, &arn).await
            } else {
                create_assignment(&router, &arn, "USER", "f81d4fae-7dec-11d0-a765-00a0c91e6bf6")
                    .await
            };
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: an unminted ARN must be refused 400, got \
                 {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got \
                 {body}"
            );
        }
        let (status, body) =
            describe_assignment(&router, &arn, "USER", "f81d4fae-7dec-11d0-a765-00a0c91e6bf6")
                .await;
        assert_eq!(
            status, 400,
            "case {case:?} on DescribeApplicationAssignment: an unminted ARN must be \
             refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?} on DescribeApplicationAssignment: expected \
             ResourceNotFoundException, got {body}"
        );
    }

    // Not one of those refusals granted anything.
    assert_eq!(
        with_store(&engine, |store| store.application_assignments.len()),
        0,
        "a refused grant must store no assignment"
    );

    // The stored application survived every refusal.
    let (status, body) = describe_application(&router, &stored).await;
    assert_eq!(
        status, 200,
        "the stored application must survive the refusals, got {status} {body}"
    );
}

/// Rule 4, the cascade: an application lives inside its instance, so deleting the
/// instance takes the application and the assignments to it with it — the grant is not
/// inherited by anything created later.
#[tokio::test]
async fn an_application_and_its_assignments_stop_existing_with_their_instance() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = minted_application(
        &router,
        &instance_arn,
        &Application::new("an application of the deleted instance", "doomed-app"),
    )
    .await;
    let principal_id = "f81d4fae-7dec-11d0-a765-00a0c91e6bf6";
    let (status, body) = create_assignment(&router, &application_arn, "USER", principal_id).await;
    assert_eq!(status, 200, "the grant must answer 200, got {status} {body}");

    let (status, body) = call(
        &router,
        "DeleteInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteInstance must answer 200, got {status} {body}"
    );
    // The deletion falls due a span after the call; a test moves the stored due instant
    // rather than sleeping for it.
    hold_deletion(
        &engine,
        std::time::SystemTime::now() - std::time::Duration::from_secs(60),
    );

    let (status, body) = describe_application(&router, &application_arn).await;
    assert_eq!(
        status, 400,
        "an application of a deleted instance must be gone, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {body}"
    );
    let (status, body) = describe_assignment(&router, &application_arn, "USER", principal_id).await;
    assert_eq!(
        status, 400,
        "an assignment to a gone application must be gone, got {status} {body}"
    );
    assert_eq!(
        with_store(&engine, |store| store.application_assignments.len()),
        0,
        "the assignments of a deleted instance's applications must be gone too"
    );
}
