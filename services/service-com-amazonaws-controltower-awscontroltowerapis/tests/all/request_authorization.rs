// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven end to end over the real wire path (a
//! restJson1 request through the multi-service router into the mounted service), so what
//! is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: every operation of AWS Control Tower whose model declares
//! `AccessDeniedException` (HTTP 403) evaluates the identity-based IAM policies in force
//! on the principal its credentials name against `controltower:<OperationName>` BEFORE it
//! reads or writes anything. An explicit `Deny` about the request refuses it whatever an
//! `Allow` says; a request no readable statement allows falls to IAM's default deny; and
//! the refusal names the signing principal, the action and the resource the request
//! addressed. A request a matching `Allow` covers reaches the operation's own answer.
//!
//! The resource a request is judged against is the ARN it NAMES when it names one of this
//! service's, and otherwise the collection of the resource type the operation acts on —
//! `<type>/*` in the Region the caller signed for — which is what IAM evaluates for a
//! create, a listing or a report over a whole resource type.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME
//! shared engine — the stores those services' own handlers write — so the documents under
//! test are the ones `iam:PutRolePolicy` stores and the principal a refusal blames is the
//! one `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The tables span what the rule generalises over: the operation (every one of them, read
//! off the MODEL rather than a second hand list), the documents in force (none at all,
//! another action of this service, another service's actions, an allow and a deny in two
//! separate inline policies, an allow and a deny in one, an allow of exactly the action, of
//! the action in another case, of the whole service and of everything), the resource an
//! allow is scoped to (every resource, this account's resources of the type, exactly the
//! one named, another one, another Region's) and the principal (an assumed-role session and
//! the account's own published credential).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_controltower_awscontroltowerapis::state;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{control, ou};
use crate::landing_zone_lifecycle::{
    call, dispatch_as, set_up_account, simple_manifest, Answer, ACCOUNT_ACCESS_KEY_ID,
    SIGNED_REGION,
};

/// This service's Smithy model, as it ships beside this crate. Read at test time rather
/// than embedded: it is what says which operations the rule covers.
const MODEL_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-controltower-awscontroltowerapis.json"
);

/// The error code every authorization refusal carries, and the shape the model declares it
/// as.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The access key id of the assumed-role session every row signs with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, and the session's own name.
const ROLE: &str = "controltower-reader";
const SESSION: &str = "controltower-session";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// A router serving AWS Control Tower beside the IAM and STS services, over ONE shared
/// engine, so the policies and the session the cases seed are the ones those services hold.
fn emulator_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_controltower_awscontroltowerapis::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        ::engine::emulator_account_id()
    )
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE}",
        ::engine::emulator_account_id()
    )
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let now = SystemTime::now();
    let account = ::engine::emulator_account_id();
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLECONTROLTOWER1".to_string(),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: account.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the session is recorded");
}

/// Put `documents` in force on the role as its inline policies, one policy per document,
/// replacing whatever it carried — what a sequence of `iam:PutRolePolicy` calls does. An
/// empty slice takes every policy off it, as `iam:DeleteRolePolicy` does.
fn given_role_policies(engine: &Arc<engine::InMemoryEngine>, documents: &[Value]) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    if documents.is_empty() {
        store.role_inline_policies.remove(&iam::name_key(ROLE));
        return;
    }
    let policies: BTreeMap<String, iam::InlinePolicy> = documents
        .iter()
        .enumerate()
        .map(|(index, document)| {
            let policy_name = format!("inline-{index}");
            (
                iam::name_key(&policy_name),
                iam::InlinePolicy {
                    policy_name,
                    policy_document: document.to_string(),
                },
            )
        })
        .collect();
    store
        .role_inline_policies
        .insert(iam::name_key(ROLE), policies);
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// A policy denying `actions` on every resource.
fn deny(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Deny", "Action": actions, "Resource": "*" }]
    })
}

/// Which of IAM's refusals a row expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The default deny of a request no readable statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with.
    fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// The documents a row puts in force on the role, and which of IAM's refusals the request
/// then meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(
    action: &str,
    other: &str,
) -> Vec<(&'static str, Vec<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", Vec::new(), Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            vec![allow(json!(other))],
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            vec![allow(json!("s3:*"))],
            Some(Refusal::DefaultDeny),
        ),
        (
            // The recorded case: one inline policy allows the action and a SECOND inline
            // policy denies it, so the deny has to win across documents.
            "a second inline policy denying exactly this action beside one allowing it",
            vec![allow(json!(action)), deny(json!(action))],
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            vec![json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": "controltower:*", "Resource": "*" },
                ],
            })],
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            vec![allow(json!(action))],
            None,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow is
            // not what decides the request.
            "an allow of this action spelled in lower case",
            vec![allow(json!(action.to_lowercase()))],
            None,
        ),
        (
            "an allow listing this action beside another service's",
            vec![allow(json!(["s3:GetObject", action]))],
            None,
        ),
        (
            "an allow of every action of this service",
            vec![allow(json!("controltower:*"))],
            None,
        ),
        ("an allow of every action", vec![allow(json!("*"))], None),
    ]
}

/// One operation held to the rule: a request the model accepts, and the resource the gate
/// judges that request against.
struct Judged {
    /// The operation's own name, which is the second half of its IAM action.
    operation: &'static str,
    /// The `@http` path the operation is bound to.
    path: &'static str,
    /// A request satisfying every modelled constraint, so this service's front door does
    /// not refuse it ahead of the gate.
    request: Value,
    /// The ARN the refusal must name: the one the request addresses.
    resource: String,
}

/// An `operationIdentifier` of the modelled form
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), for the reporting
/// operations that address an asynchronous operation rather than a resource.
const AN_OPERATION_IDENTIFIER: &str = "11111111-2222-3333-4444-555555555555";

/// Every operation of this service that models `AccessDeniedException`, with the request it
/// is driven with and the resource IAM judges that request against.
///
/// A named identifier is an ARN of the form this service mints for that resource type, so
/// the gate judges the request against the ARN it carries; an operation that names no single
/// resource is judged against the collection of its type in the signed Region.
fn judged() -> Vec<Judged> {
    let landing_zone = state::landing_zone_arn(SIGNED_REGION, "1a2b3c4d5e");
    let landing_zones = state::landing_zone_arn(SIGNED_REGION, "*");
    let enabled_control = state::enabled_control_arn(SIGNED_REGION, "9z8y7x6w5v");
    let enabled_controls = state::enabled_control_arn(SIGNED_REGION, "*");
    let enabled_baseline = state::enabled_baseline_arn(SIGNED_REGION, "4d5e6f7g8h");
    let enabled_baselines = state::enabled_baseline_arn(SIGNED_REGION, "*");
    let baseline = state::baseline_arn(SIGNED_REGION, state::BASELINE_CATALOG[0].id);
    let baselines = state::baseline_arn(SIGNED_REGION, "*");

    vec![
        Judged {
            operation: "ListLandingZones",
            path: "/list-landingzones",
            request: json!({}),
            resource: landing_zones.clone(),
        },
        Judged {
            operation: "CreateLandingZone",
            path: "/create-landingzone",
            request: json!({ "version": "3.3", "manifest": simple_manifest() }),
            resource: landing_zones.clone(),
        },
        Judged {
            operation: "GetLandingZone",
            path: "/get-landingzone",
            request: json!({ "landingZoneIdentifier": landing_zone }),
            resource: landing_zone.clone(),
        },
        Judged {
            operation: "UpdateLandingZone",
            path: "/update-landingzone",
            request: json!({
                "landingZoneIdentifier": landing_zone,
                "version": "3.3",
                "manifest": simple_manifest(),
            }),
            resource: landing_zone.clone(),
        },
        Judged {
            operation: "ResetLandingZone",
            path: "/reset-landingzone",
            request: json!({ "landingZoneIdentifier": landing_zone }),
            resource: landing_zone.clone(),
        },
        Judged {
            operation: "DeleteLandingZone",
            path: "/delete-landingzone",
            request: json!({ "landingZoneIdentifier": landing_zone }),
            resource: landing_zone.clone(),
        },
        Judged {
            operation: "GetLandingZoneOperation",
            path: "/get-landingzone-operation",
            request: json!({ "operationIdentifier": AN_OPERATION_IDENTIFIER }),
            resource: landing_zones.clone(),
        },
        Judged {
            operation: "ListLandingZoneOperations",
            path: "/list-landingzone-operations",
            request: json!({}),
            resource: landing_zones,
        },
        Judged {
            operation: "EnableControl",
            path: "/enable-control",
            request: json!({
                "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
                "targetIdentifier": ou("1a2b"),
            }),
            resource: enabled_controls.clone(),
        },
        Judged {
            operation: "DisableControl",
            path: "/disable-control",
            request: json!({ "enabledControlIdentifier": enabled_control }),
            resource: enabled_control.clone(),
        },
        Judged {
            operation: "GetEnabledControl",
            path: "/get-enabled-control",
            request: json!({ "enabledControlIdentifier": enabled_control }),
            resource: enabled_control.clone(),
        },
        Judged {
            operation: "UpdateEnabledControl",
            path: "/update-enabled-control",
            request: json!({
                "enabledControlIdentifier": enabled_control,
                "parameters": [{ "key": "AllowedRegions", "value": ["us-west-2"] }],
            }),
            resource: enabled_control.clone(),
        },
        Judged {
            operation: "ResetEnabledControl",
            path: "/reset-enabled-control",
            request: json!({ "enabledControlIdentifier": enabled_control }),
            resource: enabled_control,
        },
        Judged {
            operation: "ListEnabledControls",
            path: "/list-enabled-controls",
            request: json!({}),
            resource: enabled_controls.clone(),
        },
        Judged {
            operation: "GetControlOperation",
            path: "/get-control-operation",
            request: json!({ "operationIdentifier": AN_OPERATION_IDENTIFIER }),
            resource: enabled_controls.clone(),
        },
        Judged {
            operation: "ListControlOperations",
            path: "/list-control-operations",
            request: json!({}),
            resource: enabled_controls,
        },
        Judged {
            operation: "EnableBaseline",
            path: "/enable-baseline",
            request: json!({
                "baselineIdentifier": baseline,
                "baselineVersion": "1.0",
                "targetIdentifier": ou("3c4d"),
            }),
            resource: enabled_baselines.clone(),
        },
        Judged {
            operation: "DisableBaseline",
            path: "/disable-baseline",
            request: json!({ "enabledBaselineIdentifier": enabled_baseline }),
            resource: enabled_baseline.clone(),
        },
        Judged {
            operation: "GetEnabledBaseline",
            path: "/get-enabled-baseline",
            request: json!({ "enabledBaselineIdentifier": enabled_baseline }),
            resource: enabled_baseline.clone(),
        },
        Judged {
            operation: "UpdateEnabledBaseline",
            path: "/update-enabled-baseline",
            request: json!({
                "enabledBaselineIdentifier": enabled_baseline,
                "baselineVersion": "1.0",
            }),
            resource: enabled_baseline.clone(),
        },
        Judged {
            operation: "ResetEnabledBaseline",
            path: "/reset-enabled-baseline",
            request: json!({ "enabledBaselineIdentifier": enabled_baseline }),
            resource: enabled_baseline,
        },
        Judged {
            operation: "ListEnabledBaselines",
            path: "/list-enabled-baselines",
            request: json!({}),
            resource: enabled_baselines.clone(),
        },
        Judged {
            operation: "GetBaselineOperation",
            path: "/get-baseline-operation",
            request: json!({ "operationIdentifier": AN_OPERATION_IDENTIFIER }),
            resource: enabled_baselines,
        },
        Judged {
            operation: "GetBaseline",
            path: "/get-baseline",
            request: json!({ "baselineIdentifier": baseline }),
            resource: baseline,
        },
        Judged {
            operation: "ListBaselines",
            path: "/list-baselines",
            request: json!({}),
            resource: baselines,
        },
    ]
}

/// Every operation the MODEL declares `AccessDeniedException` for, read out of the model
/// that generated this crate.
fn operations_modelling_access_denied() -> Vec<String> {
    let model = std::fs::read_to_string(MODEL_PATH)
        .expect("the model that generated this crate ships beside it");
    let model: Value =
        serde_json::from_str(&model).expect("the model that generated this crate is JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("a Smithy model declares shapes")
        .clone();
    shapes
        .iter()
        .filter(|(_, shape)| shape["type"] == "operation")
        .filter(|(_, shape)| {
            shape["errors"]
                .as_array()
                .is_some_and(|errors| {
                    errors.iter().any(|error| {
                        error["target"]
                            .as_str()
                            .is_some_and(|target| target.ends_with(&format!("#{ACCESS_DENIED}")))
                    })
                })
        })
        .filter_map(|(id, _)| id.rsplit('#').next().map(str::to_string))
        .collect()
}

/// The refusal AWS words an authorization failure with, built from the request: the signing
/// principal, the action and the resource the request addressed.
fn expected_message(resource: &str, action: &str, refusal: Refusal) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} {}",
        session_arn(),
        refusal.reason(action)
    )
}

/// THE RULE, over the WHOLE surface: every operation that models `AccessDeniedException` is
/// judged against its own `controltower:<OperationName>` action and the resource its request
/// addresses, before it performs anything; a refusal is HTTP 403 `AccessDeniedException`
/// naming the principal, the action, the resource and which of IAM's rules refused it, and
/// an allowed call is not refused by authorization at all.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action_and_resource() {
    let surface = judged();
    let actions: Vec<String> = surface
        .iter()
        .map(|judged| format!("controltower:{}", judged.operation))
        .collect();
    for judged in &surface {
        let action = format!("controltower:{}", judged.operation);
        // Another action of this service, which a row grants instead of the one under test.
        let other = actions
            .iter()
            .find(|candidate| **candidate != action)
            .expect("this service has more than one operation")
            .clone();

        for (row, documents, refused) in documents_in_force(&action, &other) {
            let case = format!("{} with {row}", judged.operation);
            // A fresh store, role and session per row: no row can pass on another's write.
            let (router, engine) = emulator_with_identity();
            given_session(&engine);
            given_role_policies(&engine, &documents);

            let answer = call_as_session(&router, judged.path, judged.request.clone()).await;
            match refused {
                Some(refusal) => {
                    assert_eq!(
                        (answer.status, answer.error_code.as_deref()),
                        (403, Some(ACCESS_DENIED)),
                        "case {case:?}: expected 403 {ACCESS_DENIED}, got {} {:?} body {:?}",
                        answer.status,
                        answer.error_code,
                        answer.body
                    );
                    assert_eq!(
                        answer.string("message"),
                        expected_message(&judged.resource, &action, refusal),
                        "case {case:?}: expected IAM's wording, got {:?}",
                        answer.string("message")
                    );
                }
                None => {
                    assert_ne!(
                        answer.error_code.as_deref(),
                        Some(ACCESS_DENIED),
                        "case {case:?}: an allowed request must reach the operation's own \
                         answer, got {} body {:?}",
                        answer.status,
                        answer.body
                    );
                    assert_ne!(
                        answer.status, 403,
                        "case {case:?}: an allowed request must not be refused, got body {:?}",
                        answer.body
                    );
                }
            }
        }
    }
}

/// RULE applied to the whole surface: the operations held to the rule above are exactly the
/// ones the MODEL declares `AccessDeniedException` for — an operation whose model declares
/// no 403 is not judged, because answering a caller an error its operation does not model
/// would be a different divergence.
///
/// The names come from the model rather than from a second hand list, so an operation that
/// gains the error and not the gate fails here.
#[test]
fn the_rule_covers_every_operation_that_models_the_refusal() {
    let modelled = operations_modelling_access_denied();
    assert!(
        modelled.len() > 20,
        "case the model's surface: expected this service's operations, got {modelled:?}"
    );
    let judged: Vec<&str> = judged().iter().map(|judged| judged.operation).collect();
    for operation in &modelled {
        assert!(
            judged.contains(&operation.as_str()),
            "case {operation}: the model declares {ACCESS_DENIED} for it, so it is judged by \
             the gate and belongs in this suite's table; the table holds {judged:?}"
        );
    }
    for operation in &judged {
        assert!(
            modelled.contains(&(*operation).to_string()),
            "case {operation}: this suite holds it to the rule, but the model declares no \
             {ACCESS_DENIED} for it"
        );
    }
}

/// Call one operation signed with the seeded session's key, which is what names the
/// principal the request speaks for.
async fn call_as_session(router: &MultiServiceRouter, path: &str, body: Value) -> Answer {
    dispatch_as(
        router,
        SESSION_KEY_ID,
        SIGNED_REGION,
        "POST",
        path,
        Some(body),
    )
    .await
}

/// RULE: the decision is made against the resource the request ADDRESSES, so an allow
/// scoped to that ARN or to a pattern covering it grants the call, and one scoped to
/// another resource of the same type, or to another Region's, does not.
#[tokio::test]
async fn an_allow_is_scoped_to_the_resource_the_request_addresses() {
    let account = ::engine::emulator_account_id();
    let addressed = state::landing_zone_arn(SIGNED_REGION, "1a2b3c4d5e");
    let another = state::landing_zone_arn(SIGNED_REGION, "f6g7h8i9j0");

    // (case, the Resource the allow is scoped to, whether it grants the call)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("every resource", json!("*"), true),
        ("exactly the landing zone addressed", json!(addressed), true),
        (
            "every landing zone of this account in this Region",
            json!(state::landing_zone_arn(SIGNED_REGION, "*")),
            true,
        ),
        (
            "every resource of this service in this Region",
            json!(format!("arn:aws:controltower:{SIGNED_REGION}:{account}:*")),
            true,
        ),
        ("another landing zone only", json!(another), false),
        (
            "every landing zone of another Region",
            json!(state::landing_zone_arn("eu-west-1", "*")),
            false,
        ),
        (
            "every resource of another service",
            json!("arn:aws:s3:::*"),
            false,
        ),
    ];

    for (case, resource, granted) in rows {
        let (router, engine) = emulator_with_identity();
        given_session(&engine);
        given_role_policies(
            &engine,
            &[json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "controltower:GetLandingZone",
                    "Resource": resource,
                }],
            })],
        );

        let answer = call_as_session(
            &router,
            "/get-landingzone",
            json!({ "landingZoneIdentifier": addressed }),
        )
        .await;
        // No landing zone was ever created, so every row answers a client error;
        // authorization answers BEFORE the store is read, so an allowed call falls through
        // to the operation's own verdict for a landing zone nothing created and a refused
        // one never gets that far — which is also what keeps a refusal from disclosing
        // whether the landing zone exists.
        let expected = if granted {
            "ResourceNotFoundException"
        } else {
            ACCESS_DENIED
        };
        assert_eq!(
            answer.error_code.as_deref(),
            Some(expected),
            "case {case:?}: expected {expected}, got {} {:?} body {:?}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }
}

/// RULE: a listing addresses the whole collection, so an allow scoped to one resource of
/// the type does not cover it while one scoped to the collection does.
#[tokio::test]
async fn a_listing_is_judged_against_the_whole_collection() {
    let one = state::landing_zone_arn(SIGNED_REGION, "1a2b3c4d5e");
    let collection = state::landing_zone_arn(SIGNED_REGION, "*");

    // (case, the Resource the allow is scoped to, whether it grants the listing)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("one landing zone", json!(one), false),
        ("every landing zone of this Region", json!(collection), true),
        ("every resource", json!("*"), true),
    ];

    for (case, resource, granted) in rows {
        let (router, engine) = emulator_with_identity();
        given_session(&engine);
        given_role_policies(
            &engine,
            &[json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "controltower:ListLandingZones",
                    "Resource": resource,
                }],
            })],
        );

        let answer = call_as_session(&router, "/list-landingzones", json!({})).await;
        if granted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the listing to be served, got {} body {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (403, Some(ACCESS_DENIED)),
                "case {case:?}: expected 403 {ACCESS_DENIED}, got {} body {:?}",
                answer.status,
                answer.body
            );
        }
    }
}

/// RULE: an explicit `Deny` is what refuses — attaching it to a principal an `Allow`
/// already covers turns the identical signed call from served into refused, and taking it
/// off again restores the service, so the refusal is the deny's and not an absent grant's.
#[tokio::test]
async fn an_explicit_deny_refuses_a_call_an_allow_already_served() {
    let (router, engine) = emulator_with_identity();
    given_session(&engine);
    // The account's own published credential sets the account up, so the listing under test
    // has something to report.
    let landing_zone = set_up_account(&router).await;

    let action = "controltower:ListLandingZones";
    let allowed = allow(json!(action));

    // The positive control: with the allow alone the session is served, and the listing is
    // the one the account holds.
    given_role_policies(&engine, std::slice::from_ref(&allowed));
    let served = call_as_session(&router, "/list-landingzones", json!({})).await;
    assert_eq!(
        served.status, 200,
        "case \"the allow alone\": expected the listing to be served, got {} body {:?}",
        served.status, served.body
    );
    assert_eq!(
        served.get("landingZones")[0]["arn"].as_str(),
        Some(landing_zone.as_str()),
        "case \"the allow alone\": expected the account's landing zone, got {:?}",
        served.body
    );

    // The deny, attached as a SECOND inline policy: the identical call is now refused, with
    // IAM's explicit-deny wording naming the principal, the action and the collection the
    // listing addresses.
    given_role_policies(&engine, &[allowed.clone(), deny(json!(action))]);
    let refused = call_as_session(&router, "/list-landingzones", json!({})).await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (403, Some(ACCESS_DENIED)),
        "case \"the deny attached\": expected 403 {ACCESS_DENIED}, got {} body {:?}",
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.string("message"),
        expected_message(
            &state::landing_zone_arn(SIGNED_REGION, "*"),
            action,
            Refusal::ExplicitDeny
        ),
        "case \"the deny attached\": expected IAM's wording, got {:?}",
        refused.string("message")
    );
    assert!(
        refused.get("landingZones").is_null(),
        "case \"the deny attached\": a refused call reports nothing of the listing, got {:?}",
        refused.body
    );

    // The deny deleted: the identical call is served again, which is what says the refusal
    // was the deny's.
    given_role_policies(&engine, std::slice::from_ref(&allowed));
    let restored = call_as_session(&router, "/list-landingzones", json!({})).await;
    assert_eq!(
        restored.status, 200,
        "case \"the deny deleted\": expected the listing to be served again, got {} body {:?}",
        restored.status, restored.body
    );
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds a record
/// for — is not a principal the IAM store holds policies for, so it is served whatever the
/// role's policies say.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    let (router, engine) = emulator_with_identity();
    given_session(&engine);
    // The role carries nothing at all, which is what refuses the SESSION above.
    given_role_policies(&engine, &[]);

    for path in [
        "/list-landingzones",
        "/list-baselines",
        "/list-landingzone-operations",
        "/list-control-operations",
    ] {
        let answer = call(&router, path, json!({})).await;
        assert_eq!(
            answer.status, 200,
            "case {path:?}: the account's own credential ({ACCOUNT_ACCESS_KEY_ID}) must be \
             served, got {} body {:?}",
            answer.status, answer.body
        );
    }
}
