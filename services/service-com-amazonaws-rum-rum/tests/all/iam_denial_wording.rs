// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS an authorization refusal, across the operations that render one
//! and the denial classes it has to tell apart.
//!
//! The rule under test: when IAM withholds the grant, the call is refused with
//! `AccessDeniedException` (HTTP 403) whose message names the principal the request's
//! credential speaks for, the action it asked for and the resource the decision was taken
//! on, and ends in a clause naming the KIND of policy that took the decision — an implicit
//! deny, where no statement of the caller's identity-based policies allowed the action,
//! ends in "because no identity-based policy allows the <action> action", and an explicit
//! `Deny` ends in "with an explicit deny in an identity-based policy" or "with an explicit
//! deny in a resource-based policy", for whichever policy carries it. That clause is the
//! only discriminator a client has between the denial classes, so each class words its own
//! and none of them is the bare "no policy" phrasing.
//!
//! One sentence for the whole service: the operation changes only the action and the
//! resource the sentence names, which is why the table below is operations × denial
//! classes rather than one case per operation.
//!
//! The table spans what the rule generalises over: the operation (and so the action, which
//! the implicit-deny form names twice), the denial class, and the kind of principal the
//! credential speaks for — an STS session, named by the session's own ARN, and an IAM
//! user, named by the user's. The positive control at the end keeps the table honest: a
//! principal allowed exactly the row's action is not refused at all, so the table cannot
//! pass by everything being denied.
//!
//! Every case drives the real wire path through the shared harness in
//! `tests/all/authorization.rs`: the principals are made with IAM's and STS's own calls and
//! the RUM request carries the credential's access key id in its SigV4 credential scope.
//!
//! `PutRumEvents` is not a row: its refusal names the app monitor by the `Id` the request
//! carried rather than by the ARN this table asserts, which is the subject of
//! `data_plane_denial_identifier.rs`, and every assertion here is on a whole message.

use serde_json::{json, Value};

use crate::authorization::{
    account, identity_policy, monitor_arn, resource_policy, statement, Emulator, ACCOUNT_KEY,
};

/// The status and wire code every refusal below answers with, as the model declares for
/// `AccessDeniedException` (`errorKind` client, `httpError` 403).
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// An action of this service that no row below is authorized against, for the caller that
/// holds a grant for something else entirely.
const UNRELATED_ACTION: &str = "rum:ListAppMonitors";

/// Which clause a refusal ends in: the kind of policy that took the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    /// No statement of the caller's identity-based policies allowed the action.
    NoIdentityPolicy,
    /// A statement of the caller's identity-based policies denied it outright.
    IdentityDeny,
    /// A statement of the resource's own policy denied it outright.
    ResourceDeny,
}

impl Clause {
    /// The clause as IAM words it for `action`.
    fn text(self, action: &str) -> String {
        match self {
            Self::NoIdentityPolicy => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::IdentityDeny => "with an explicit deny in an identity-based policy".to_string(),
            Self::ResourceDeny => "with an explicit deny in a resource-based policy".to_string(),
        }
    }
}

/// One operation that renders the sentence: the action it is authorized against, and a
/// request that reaches that decision.
///
/// Source: `smithy_query(query="operation", operation="<name>")` for each `http`
/// method/uri, and this crate's `OPERATIONS.md` for the action names.
struct OperationCase {
    action: &'static str,
    method: &'static str,
    /// `{name}` stands for the row's own app monitor name and `{definition}` for the metric
    /// definition id the case addresses.
    path: &'static str,
    /// The request body, built for that same metric definition id.
    body: fn(&str) -> Option<Value>,
    /// What an AUTHORIZED call needs the monitor to hold already for its answer to be this
    /// row's [`OperationCase::allowed_status`].
    seed: Seed,
    /// The status an authorized caller gets for this request once [`OperationCase::seed`]
    /// is in place: the operation's own success status, or the 404 the model declares for
    /// what the request names and the monitor does not hold.
    allowed_status: u16,
}

/// What an authorized call needs in place before it can succeed: a metric definition lives
/// in a destination, and a destination has to be put before either exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seed {
    /// Nothing: the request addresses the app monitor itself.
    Nothing,
    /// The `CloudWatch` extended-metrics destination.
    Destination,
    /// That destination with one metric definition stored in it, whose id the request then
    /// addresses.
    DestinationAndDefinition,
}

/// `ResourceNotFoundException` and `PolicyNotFoundException` are both `httpError` 404
/// (`smithy_query(query="operation", …)`), which is what an authorized call naming
/// something the monitor does not hold answers with.
const NOT_FOUND_STATUS: u16 = 404;

/// The status every operation below declares for success (`http.code`).
const OK_STATUS: u16 = 200;

/// The metric definition id the refusal table addresses: nothing is stored under it, which
/// is immaterial there because the authorization decision is taken first.
const UNSTORED_DEFINITION_ID: &str = "no-such-definition";

/// One metric definition the service accepts, so a row carrying one is refused by the
/// authorization decision rather than by the definition's own validation.
///
/// Source: `smithy_query(query="shape", shape="MetricDefinitionRequest")` — `SessionCount`
/// is one of the extended metrics `Name` admits, and an extended metric's `EventPattern`
/// has to name the event it counts.
fn extended_metric() -> Value {
    json!({
        "Name": "SessionCount",
        "EventPattern": "{\"event_type\":[\"com.amazon.rum.session_start_event\"]}",
    })
}

/// Every operation of this service that is authorized against ONE app monitor and renders
/// the sentence, each with a request the authorization decision is reached on.
fn operation_cases() -> Vec<OperationCase> {
    vec![
        OperationCase {
            action: "rum:GetAppMonitor",
            method: "GET",
            path: "/appmonitor/{name}",
            body: |_| None,
            seed: Seed::Nothing,
            allowed_status: OK_STATUS,
        },
        OperationCase {
            action: "rum:UpdateAppMonitor",
            method: "PATCH",
            path: "/appmonitor/{name}",
            body: |_| Some(json!({"Domain": "updated.example.com"})),
            seed: Seed::Nothing,
            allowed_status: OK_STATUS,
        },
        OperationCase {
            action: "rum:DeleteAppMonitor",
            method: "DELETE",
            path: "/appmonitor/{name}",
            body: |_| None,
            seed: Seed::Nothing,
            allowed_status: OK_STATUS,
        },
        OperationCase {
            action: "rum:GetResourcePolicy",
            method: "GET",
            path: "/appmonitor/{name}/policy",
            body: |_| None,
            seed: Seed::Nothing,
            // The monitor carries no resource policy, which is this operation's own
            // modelled `PolicyNotFoundException` rather than a refusal.
            allowed_status: NOT_FOUND_STATUS,
        },
        OperationCase {
            action: "rum:PutRumMetricsDestination",
            method: "POST",
            path: "/rummetrics/{name}/metricsdestination",
            body: |_| Some(json!({"Destination": "CloudWatch"})),
            seed: Seed::Nothing,
            allowed_status: OK_STATUS,
        },
        OperationCase {
            action: "rum:ListRumMetricsDestinations",
            method: "GET",
            path: "/rummetrics/{name}/metricsdestination",
            body: |_| None,
            seed: Seed::Destination,
            allowed_status: OK_STATUS,
        },
        OperationCase {
            action: "rum:DeleteRumMetricsDestination",
            method: "DELETE",
            path: "/rummetrics/{name}/metricsdestination?destination=CloudWatch",
            body: |_| None,
            // The destination this call removes has to be there, so the success is an
            // observable removal rather than a no-op.
            seed: Seed::Destination,
            allowed_status: OK_STATUS,
        },
        OperationCase {
            action: "rum:BatchCreateRumMetricDefinitions",
            method: "POST",
            path: "/rummetrics/{name}/metrics",
            body: |_| {
                Some(json!({
                    "Destination": "CloudWatch",
                    "MetricDefinitions": [extended_metric()],
                }))
            },
            // A definition is created IN a destination, so the destination has to be put
            // first; the definition itself is what this call stores.
            seed: Seed::Destination,
            allowed_status: OK_STATUS,
        },
        OperationCase {
            action: "rum:BatchGetRumMetricDefinitions",
            method: "GET",
            path: "/rummetrics/{name}/metrics?destination=CloudWatch",
            body: |_| None,
            seed: Seed::DestinationAndDefinition,
            allowed_status: OK_STATUS,
        },
        OperationCase {
            action: "rum:BatchDeleteRumMetricDefinitions",
            method: "DELETE",
            path: "/rummetrics/{name}/metrics?destination=CloudWatch&metricDefinitionIds={definition}",
            body: |_| None,
            // The definition this call deletes has to be stored for the success to be an
            // observable deletion.
            seed: Seed::DestinationAndDefinition,
            allowed_status: OK_STATUS,
        },
        OperationCase {
            action: "rum:UpdateRumMetricDefinition",
            method: "PATCH",
            path: "/rummetrics/{name}/metrics",
            body: |definition| {
                Some(json!({
                    "Destination": "CloudWatch",
                    "MetricDefinitionId": definition,
                    "MetricDefinition": extended_metric(),
                }))
            },
            // The definition this call rewrites has to be stored.
            seed: Seed::DestinationAndDefinition,
            allowed_status: OK_STATUS,
        },
    ]
}

/// One denial class: which policies are in force, and the clause that answers for them.
struct DenialCase {
    case: &'static str,
    /// The caller's inline identity-based policy document, built for the row's action.
    identity: fn(&str) -> Option<String>,
    /// The app monitor's resource-based policy document, built for the row's action.
    resource: fn(&str) -> Option<String>,
    clause: Clause,
}

/// The denial classes the sentence has to tell apart: an implicit deny reached two ways,
/// and an explicit `Deny` from each kind of policy.
fn denial_cases() -> Vec<DenialCase> {
    vec![
        DenialCase {
            case: "a principal holding no policy at all",
            identity: |_| None,
            resource: |_| None,
            clause: Clause::NoIdentityPolicy,
        },
        DenialCase {
            case: "an identity policy allowing another action of this service",
            identity: |_| Some(identity_policy(&statement("Allow", UNRELATED_ACTION, "*"))),
            resource: |_| None,
            clause: Clause::NoIdentityPolicy,
        },
        DenialCase {
            case: "an identity policy allowing the service and denying this action",
            identity: |action| {
                Some(identity_policy(&format!(
                    "{},{}",
                    statement("Allow", "rum:*", "*"),
                    statement("Deny", action, "*")
                )))
            },
            resource: |_| None,
            clause: Clause::IdentityDeny,
        },
        DenialCase {
            case: "an identity policy that allows and a resource policy that denies",
            identity: |_| Some(identity_policy(&statement("Allow", "rum:*", "*"))),
            resource: |action| Some(resource_policy("Deny", "*", action, None)),
            clause: Clause::ResourceDeny,
        },
    ]
}

/// The kind of principal the signing credential speaks for, which decides how the refusal
/// spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrincipalKind {
    /// An STS session over an IAM role: `sts:GetCallerIdentity` reports the session ARN,
    /// which the harness takes the session name `probe` in.
    Session,
    /// An IAM user signing with a long-term key it holds.
    User,
}

impl PrincipalKind {
    /// A fragment naming this kind, for the case label and the resource names.
    fn slug(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::User => "user",
        }
    }

    /// The ARN a refusal names a principal of this kind by.
    fn arn(self, name: &str) -> String {
        match self {
            Self::Session => format!("arn:aws:sts::{}:assumed-role/{name}/probe", account()),
            Self::User => format!("arn:aws:iam::{}:user/{name}", account()),
        }
    }
}

/// Every kind of principal the sentence has to spell.
const PRINCIPAL_KINDS: [PrincipalKind; 2] = [PrincipalKind::Session, PrincipalKind::User];

/// The whole sentence: the principal, the action, the resource the decision was taken on,
/// and the clause naming the policy kind that withheld the grant.
fn expected_message(principal_arn: &str, action: &str, resource: &str, clause: Clause) -> String {
    format!(
        "User: {principal_arn} is not authorized to perform: {action} on resource: {resource} {}",
        clause.text(action)
    )
}

#[tokio::test]
async fn every_operation_words_its_refusal_about_the_policy_kind_that_decided() {
    for (class_index, class) in denial_cases().iter().enumerate() {
        for kind in PRINCIPAL_KINDS {
            // One emulator per (class, principal kind): every row inside gets its own app
            // monitor and its own principal, so no row inherits another's grant.
            let emulator = Emulator::new();
            for (op_index, operation) in operation_cases().iter().enumerate() {
                let case = format!(
                    "{} refused for {} ({})",
                    operation.action,
                    class.case,
                    kind.slug()
                );
                let monitor = format!("denial-{class_index}-{}-{op_index}", kind.slug());
                emulator.create_monitor(&monitor).await;
                let identity = (class.identity)(operation.action);
                let principal = format!("denied-{class_index}-{}-{op_index}", kind.slug());
                let key = match kind {
                    PrincipalKind::Session => {
                        emulator
                            .assumed_role_key(&principal, identity.as_deref())
                            .await
                    }
                    PrincipalKind::User => {
                        emulator.iam_user_key(&principal, identity.as_deref()).await
                    }
                };
                if let Some(document) = (class.resource)(operation.action) {
                    emulator.put_resource_policy(&monitor, &document).await;
                }

                let path = operation
                    .path
                    .replace("{name}", &monitor)
                    .replace("{definition}", UNSTORED_DEFINITION_ID);
                let reply = emulator
                    .rum(
                        &key,
                        operation.method,
                        &path,
                        (operation.body)(UNSTORED_DEFINITION_ID),
                    )
                    .await;
                assert_eq!(
                    reply.status, ACCESS_DENIED_STATUS,
                    "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} {:?}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                assert_eq!(
                    reply.error_code(&case),
                    ACCESS_DENIED_CODE,
                    "case {case:?}: expected {ACCESS_DENIED_CODE}, got {:?}",
                    String::from_utf8_lossy(&reply.body)
                );
                let expected = expected_message(
                    &kind.arn(&principal),
                    operation.action,
                    &monitor_arn(&monitor),
                    class.clause,
                );
                let actual = reply.message(&case);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected the message {expected:?}, got {actual:?}"
                );
            }
        }
    }
}

/// The positive control for the table above: a principal allowed exactly the row's action,
/// with no policy denying it, is answered rather than refused — so every refusal clause
/// above is asserted about a decision that really did turn on the policies in force.
///
/// Each row is answered with its OWN modelled status, with whatever the request addresses
/// seeded first (the destination, and the definition whose id it names), so the answer is
/// the operation's real success rather than any status that merely is not the refusal.
#[tokio::test]
async fn a_principal_allowed_the_action_is_answered_rather_than_refused() {
    let emulator = Emulator::new();
    for (op_index, operation) in operation_cases().iter().enumerate() {
        let case = format!("{} allowed for a principal granted it", operation.action);
        let monitor = format!("allowed-{op_index}");
        emulator.create_monitor(&monitor).await;
        let definition = seed(&emulator, &monitor, &case, operation.seed).await;
        let key = emulator
            .assumed_role_key(
                &format!("allowed-{op_index}"),
                Some(&identity_policy(&statement("Allow", operation.action, "*"))),
            )
            .await;
        let path = operation
            .path
            .replace("{name}", &monitor)
            .replace("{definition}", &definition);
        let reply = emulator
            .rum(&key, operation.method, &path, (operation.body)(&definition))
            .await;
        assert_eq!(
            reply.status, operation.allowed_status,
            "case {case:?}: expected HTTP {}, got {} {:?}",
            operation.allowed_status,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

/// Put what `seed` asks for on `monitor` with the account's own credential, and answer the
/// id of the metric definition it stored (the unstored id when it stored none).
async fn seed(emulator: &Emulator, monitor: &str, case: &str, seed: Seed) -> String {
    if seed == Seed::Nothing {
        return UNSTORED_DEFINITION_ID.to_string();
    }
    let put = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            &format!("/rummetrics/{monitor}/metricsdestination"),
            Some(json!({"Destination": "CloudWatch"})),
        )
        .await;
    assert_eq!(
        put.status, OK_STATUS,
        "case {case:?}: seeding the destination must succeed, got {} {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    if seed == Seed::Destination {
        return UNSTORED_DEFINITION_ID.to_string();
    }
    let created = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            &format!("/rummetrics/{monitor}/metrics"),
            Some(json!({
                "Destination": "CloudWatch",
                "MetricDefinitions": [extended_metric()],
            })),
        )
        .await;
    assert_eq!(
        created.status, OK_STATUS,
        "case {case:?}: seeding the definition must succeed, got {} {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    // The id the seed minted is read back through `BatchGetRumMetricDefinitions`, the
    // operation that reports a stored definition.
    let page = emulator
        .rum(
            ACCOUNT_KEY,
            "GET",
            &format!("/rummetrics/{monitor}/metrics?destination=CloudWatch"),
            None,
        )
        .await;
    page.json(case)["MetricDefinitions"][0]["MetricDefinitionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the seed minted no MetricDefinitionId"))
        .to_string()
}
