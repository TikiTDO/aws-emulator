// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the EFFECT an executed budget action has outside Budgets: the work
//! its `Definition` describes, performed in the service that owns the resources it names.
//!
//! The RULE asserted here: executing a budget action performs, in the service that owns the
//! resources its `Definition` names, the work that definition describes — and reverting it
//! takes that same work back off — so the action's settled `ActionStatus` reports what that
//! service did with it. `Action$ActionType` "defines the type of tasks that can be carried
//! out by this action", `Action$Definition` is "where you specify all of the type-specific
//! parameters", `IamActionDefinition$PolicyArn` is "the ARN of the policy to be attached"
//! over the roles, groups and users the definition lists, and `Action$ExecutionRoleArn` is
//! "the role passed for action execution and reversion" — so an `EXECUTION_SUCCESS` asserts
//! the policy IS attached, and an execution the owning service refuses settles at the
//! `*_FAILURE` status naming the operation that did not complete.
//!
//! The effect is asserted where a CLIENT sees it: through IAM's own
//! `ListAttachedRolePolicies` / `ListAttachedGroupPolicies` / `ListAttachedUserPolicies`,
//! with IAM mounted beside budgets on ONE engine — the shape `core/router/src/lib.rs` mounts
//! services in — and every role, group, user and policy created through IAM's own
//! operations, so nothing is asserted about a record only budgets holds.
//!
//! The tables span the class rather than one request: their rows differ along the KIND of
//! identity the definition names (role, group, user, and all three at once), the NUMBER of
//! them, whether the policy is one this account created or one of AWS's own
//! (`arn:aws:iam::aws:policy/…`), whether IAM holds the identity and the policy at all, and
//! — in the second table — which execution type takes the definition back off. The last two
//! tests carry the same rule onto the other two action types, whose work AWS Organizations
//! and Systems Manager own: there the owning service cannot perform that work in this
//! emulator yet, so what each asserts is the emulator's own refusal path rather than the
//! outcome AWS produces — each says so, and names the gap.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ACTION_STATUS_EXECUTION_FAILURE, ACTION_STATUS_EXECUTION_IN_PROGRESS,
    ACTION_STATUS_EXECUTION_SUCCESS, ACTION_STATUS_PENDING, ACTION_STATUS_REVERSE_SUCCESS,
    ACTION_STATUS_STANDBY, EXECUTION_TYPE_APPROVE, EXECUTION_TYPE_RESET, EXECUTION_TYPE_REVERSE,
};

use super::budget_action_execution::{
    email_subscriber, execute_action, execution_role_arn, make_settle_due, read_status,
};
use super::budget_lifecycle::{call, caller_account};

/// The Region the in-process hops are addressed in; budgets is global and signs its
/// requests in `us-east-1`, and IAM holds one set of identities per account whichever
/// Region it is asked in.
const REGION: &str = "us-east-1";

/// The principal a role this service acts as has to trust, so the action's execution role is
/// one `CreateBudgetAction` accepts.
const BUDGETS_PRINCIPAL: &str = "budgets.amazonaws.com";

/// An identity that no row's definition names, so every table can assert that the policy
/// landed on the identities the definition NAMED and on nothing else.
const BYSTANDER_ROLE: &str = "bystander-role";

/// budgets and IAM on ONE engine, the shared engine behind them, and an IAM SDK client over
/// the router's own in-process callback — the very seam the service applies a definition
/// through.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    iam: ::aws_sdk_iam::Client,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_budgets_awsbudgetservicegateway::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    let iam = iam_client(&router);
    Emulator {
        router,
        engine,
        iam,
    }
}

/// budgets and Systems Manager on ONE engine, for the action type whose work Systems Manager
/// owns.
fn ssm_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_budgets_awsbudgetservicegateway::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// budgets and AWS Organizations on ONE engine, for the action type whose work Organizations
/// owns.
fn organizations_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_budgets_awsbudgetservicegateway::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_organizations_awsorganizationsv20161128::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// An IAM client over `router`'s own in-process callback, so the suite's setup and its reads
/// reach IAM exactly as the service under test does.
fn iam_client(router: &MultiServiceRouter) -> ::aws_sdk_iam::Client {
    let callback = EmbeddedEmulator::from_router(router.clone());
    ::aws_sdk_iam::Client::from_conf(
        ::aws_sdk_iam::Config::builder()
            .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
            .region(::aws_sdk_iam::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://iam.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled(),
            )
            .build(),
    )
}

/// Which kind of IAM identity a definition names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Role,
    Group,
    User,
}

/// One identity an action's `IamActionDefinition` names.
#[derive(Debug, Clone, Copy)]
struct Identity {
    kind: Kind,
    /// The friendly name IAM holds the identity under, which is what the definition lists.
    name: &'static str,
    /// Whether IAM holds it: a name the account holds no identity under is a name no policy
    /// can be attached to.
    held: bool,
    /// Whether the policy the action names must be attached to it once the action executed.
    attached: bool,
}

/// The managed policy an action attaches.
#[derive(Debug, Clone, Copy)]
enum Policy {
    /// One this account creates through IAM, under `name` — `created` false for a policy ARN
    /// of the same grammar that nothing created.
    Account { name: &'static str, created: bool },
    /// One of AWS's own, addressed in the `arn:aws:iam::aws:policy/` namespace every account
    /// reads and attaches from. SOURCE: the AWS Managed Policy Reference.
    AwsManaged(&'static str),
}

/// One row: an action whose definition names `identities` and attaches `policy`, and the
/// status its approval settles at.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    identities: &'static [Identity],
    policy: Policy,
    settled: &'static str,
}

/// A role the definition names, held by IAM, that the policy must land on.
const fn role(name: &'static str) -> Identity {
    Identity {
        kind: Kind::Role,
        name,
        held: true,
        attached: true,
    }
}

/// A group the definition names, held by IAM, that the policy must land on.
const fn group(name: &'static str) -> Identity {
    Identity {
        kind: Kind::Group,
        name,
        held: true,
        attached: true,
    }
}

/// A user the definition names, held by IAM, that the policy must land on.
const fn user(name: &'static str) -> Identity {
    Identity {
        kind: Kind::User,
        name,
        held: true,
        attached: true,
    }
}

/// A role the definition names that IAM holds nothing under, so nothing can be attached to
/// it.
const fn absent_role(name: &'static str) -> Identity {
    Identity {
        kind: Kind::Role,
        name,
        held: false,
        attached: false,
    }
}

/// The identity sets the tables below name. Each is a `const` so a row can borrow it for
/// the whole test rather than building it per row, and they are shared between the two
/// tables so one rule is carried over the same sets in both directions.
const ONE_ROLE: &[Identity] = &[role("dev-role")];

/// Three roles, for the row that differs only in HOW MANY identities the definition names.
const THREE_ROLES: &[Identity] = &[role("dev-role"), role("build-role"), role("ops-role")];

/// One group — the identity kind `IamActionDefinition$Groups` names.
const ONE_GROUP: &[Identity] = &[group("builders")];

/// One user — the identity kind `IamActionDefinition$Users` names.
const ONE_USER: &[Identity] = &[user("analyst")];

/// All three kinds in one definition, which is what a definition carrying all three lists
/// attaches to.
const EVERY_KIND: &[Identity] = &[
    role("dev-role"),
    role("ops-role"),
    group("builders"),
    group("reviewers"),
    user("analyst"),
];

/// One of each of the two kinds the AWS-managed policy row attaches to.
const ROLE_AND_USER: &[Identity] = &[role("dev-role"), user("analyst")];

/// One role of each kind for the reversal table: the three kinds at once.
const ROLE_GROUP_USER: &[Identity] = &[role("dev-role"), group("builders"), user("analyst")];

/// A single role IAM holds nothing under.
const ABSENT_ROLE: &[Identity] = &[absent_role("never-created")];

/// A role IAM holds beside one it does not.
const HELD_AND_ABSENT_ROLE: &[Identity] = &[role("dev-role"), absent_role("never-created")];

/// A role IAM holds, named in a definition whose POLICY nothing created, so nothing can be
/// attached to it.
const ROLE_WITHOUT_A_POLICY: &[Identity] = &[Identity {
    kind: Kind::Role,
    name: "dev-role",
    held: true,
    attached: false,
}];

/// The rows of the first table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one role",
            identities: ONE_ROLE,
            policy: Policy::Account {
                name: "DenyExpensiveLaunches",
                created: true,
            },
            settled: ACTION_STATUS_EXECUTION_SUCCESS,
        },
        Row {
            case: "three roles",
            identities: THREE_ROLES,
            policy: Policy::Account {
                name: "DenyExpensiveLaunches",
                created: true,
            },
            settled: ACTION_STATUS_EXECUTION_SUCCESS,
        },
        Row {
            case: "one group",
            identities: ONE_GROUP,
            policy: Policy::Account {
                name: "DenyEverythingElse",
                created: true,
            },
            settled: ACTION_STATUS_EXECUTION_SUCCESS,
        },
        Row {
            case: "one user",
            identities: ONE_USER,
            policy: Policy::Account {
                name: "DenyEverythingElse",
                created: true,
            },
            settled: ACTION_STATUS_EXECUTION_SUCCESS,
        },
        Row {
            case: "roles, groups and users in one definition",
            identities: EVERY_KIND,
            policy: Policy::Account {
                name: "DenyExpensiveLaunches",
                created: true,
            },
            settled: ACTION_STATUS_EXECUTION_SUCCESS,
        },
        Row {
            // The policy namespace a definition may name besides the account's own.
            case: "a policy of AWS's own catalog",
            identities: ROLE_AND_USER,
            policy: Policy::AwsManaged("arn:aws:iam::aws:policy/ReadOnlyAccess"),
            settled: ACTION_STATUS_EXECUTION_SUCCESS,
        },
        Row {
            case: "a role IAM holds nothing under",
            identities: ABSENT_ROLE,
            policy: Policy::Account {
                name: "DenyExpensiveLaunches",
                created: true,
            },
            settled: ACTION_STATUS_EXECUTION_FAILURE,
        },
        Row {
            // The definition lands on the identity IAM holds and on nothing else, so the
            // guardrail is only partly in place and the execution did not complete.
            case: "one role IAM holds beside one it does not",
            identities: HELD_AND_ABSENT_ROLE,
            policy: Policy::Account {
                name: "DenyExpensiveLaunches",
                created: true,
            },
            settled: ACTION_STATUS_EXECUTION_FAILURE,
        },
        Row {
            case: "a policy nothing created",
            identities: ROLE_WITHOUT_A_POLICY,
            policy: Policy::Account {
                name: "NeverCreated",
                created: false,
            },
            settled: ACTION_STATUS_EXECUTION_FAILURE,
        },
    ]
}

/// The ARN the row's policy is named by, created through IAM where the row says this account
/// created it.
async fn policy_arn(iam: &::aws_sdk_iam::Client, policy: Policy) -> String {
    match policy {
        Policy::AwsManaged(arn) => arn.to_string(),
        Policy::Account { name, created } if !created => {
            // The ARN grammar `PolicyArn`'s pattern states, for a policy this account holds
            // none of: `arn:aws:iam::<account>:policy/<name>`.
            format!("arn:aws:iam::{}:policy/{name}", caller_account())
        }
        Policy::Account { name, .. } => iam
            .create_policy()
            .policy_name(name)
            .policy_document(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{"Effect": "Deny", "Action": "ec2:RunInstances", "Resource": "*"}]
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap_or_else(|error| panic!("setup: IAM must create the policy {name}: {error:?}"))
            .policy()
            .and_then(|created| created.arn().map(::std::string::ToString::to_string))
            .unwrap_or_else(|| panic!("setup: IAM must report an ARN for the policy {name}")),
    }
}

/// Create every identity the row says IAM holds, plus the bystander no definition names.
async fn create_identities(iam: &::aws_sdk_iam::Client, identities: &[Identity]) {
    create_role(iam, BYSTANDER_ROLE).await;
    for identity in identities.iter().filter(|identity| identity.held) {
        match identity.kind {
            Kind::Role => create_role(iam, identity.name).await,
            Kind::Group => {
                iam.create_group()
                    .group_name(identity.name)
                    .send()
                    .await
                    .unwrap_or_else(|error| {
                        panic!("setup: IAM must create the group {}: {error:?}", identity.name)
                    });
            }
            Kind::User => {
                iam.create_user()
                    .user_name(identity.name)
                    .send()
                    .await
                    .unwrap_or_else(|error| {
                        panic!("setup: IAM must create the user {}: {error:?}", identity.name)
                    });
            }
        }
    }
}

/// Create one role through IAM, with a trust document naming nobody in particular: what the
/// rows are about is the PERMISSIONS policy an action attaches, not who may assume the role.
async fn create_role(iam: &::aws_sdk_iam::Client, role_name: &str) {
    iam.create_role()
        .role_name(role_name)
        .assume_role_policy_document(trust_document(BUDGETS_PRINCIPAL))
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: IAM must create the role {role_name}: {error:?}"));
}

/// Create the role every action below is written with — "the role passed for action
/// execution and reversion", which `CreateBudgetAction` accepts only where IAM holds it with
/// a trust document admitting this service.
async fn create_execution_role(iam: &::aws_sdk_iam::Client, account_id: &str) {
    let name = execution_role_arn(account_id)
        .rsplit('/')
        .next()
        .expect("the execution role ARN names a role")
        .to_string();
    create_role(iam, &name).await;
}

/// A trust document admitting `principal` to assume the role.
fn trust_document(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": principal},
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// The `Definition.IamActionDefinition` naming `identities` and attaching `policy_arn`. A
/// list no identity of that kind is named for is left out, the way a definition that names
/// only roles carries only `Roles`.
fn iam_definition(identities: &[Identity], policy_arn: &str) -> Value {
    let mut definition = json!({"PolicyArn": policy_arn});
    for (kind, member) in [
        (Kind::Role, "Roles"),
        (Kind::Group, "Groups"),
        (Kind::User, "Users"),
    ] {
        let named: Vec<&str> = identities
            .iter()
            .filter(|identity| identity.kind == kind)
            .map(|identity| identity.name)
            .collect();
        if !named.is_empty() {
            definition[member] = json!(named);
        }
    }
    json!({"IamActionDefinition": definition})
}

/// Seed the budget the action hangs off.
async fn seed_budget(router: &MultiServiceRouter, account_id: &str, budget_name: &str) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({
            "AccountId": account_id,
            "Budget": {
                "BudgetName": budget_name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({budget_name}) to answer 200, got {status} {body}"
    );
}

/// Create an action of `action_type` with `definition`, waiting for the approval that
/// executes it, and answer its minted `ActionId`.
///
/// A `MANUAL` `ACTUAL` action at a threshold of zero is created `PENDING` — the one status
/// `APPROVE_BUDGET_ACTION` is accepted from — because every budget seeded here reports an
/// actual spend of zero, so that threshold is already reached.
async fn seed_pending_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_type: &str,
    definition: &Value,
) -> String {
    let (status, body) = call(
        router,
        "CreateBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "NotificationType": "ACTUAL",
            "ActionType": action_type,
            "ActionThreshold": {"ActionThresholdValue": 0.0, "ActionThresholdType": "PERCENTAGE"},
            "Definition": definition,
            "ExecutionRoleArn": execution_role_arn(account_id),
            "ApprovalModel": "MANUAL",
            "Subscribers": [email_subscriber(0)]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudgetAction({budget_name}) to answer 200, got {status} {body}"
    );
    let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
    let created = read_status(router, account_id, budget_name, &action_id).await;
    assert_eq!(
        created, ACTION_STATUS_PENDING,
        "setup: expected the created action to wait for its approval, got {created:?}"
    );
    action_id
}

/// The managed policies IAM reports attached to `identity`, through IAM's own reads.
async fn attached(iam: &::aws_sdk_iam::Client, identity: Identity) -> Vec<String> {
    let mut arns: Vec<String> = match identity.kind {
        Kind::Role => iam
            .list_attached_role_policies()
            .role_name(identity.name)
            .send()
            .await
            .map(|answer| {
                answer
                    .attached_policies()
                    .iter()
                    .filter_map(|attached| {
                        attached.policy_arn().map(::std::string::ToString::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Kind::Group => iam
            .list_attached_group_policies()
            .group_name(identity.name)
            .send()
            .await
            .map(|answer| {
                answer
                    .attached_policies()
                    .iter()
                    .filter_map(|attached| {
                        attached.policy_arn().map(::std::string::ToString::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Kind::User => iam
            .list_attached_user_policies()
            .user_name(identity.name)
            .send()
            .await
            .map(|answer| {
                answer
                    .attached_policies()
                    .iter()
                    .filter_map(|attached| {
                        attached.policy_arn().map(::std::string::ToString::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };
    arns.sort();
    arns
}

/// Approving a triggered action ATTACHES the policy its definition names to every identity
/// that definition names, observable through IAM's own reads — and the action settles at the
/// status that work's outcome leaves it at.
#[tokio::test]
async fn an_executed_action_attaches_its_policy_to_every_identity_it_names() {
    let account_id = caller_account();
    for row in rows() {
        let Emulator {
            router,
            engine,
            iam,
        } = emulator();
        let case = row.case;
        create_execution_role(&iam, &account_id).await;
        create_identities(&iam, row.identities).await;
        let policy = policy_arn(&iam, row.policy).await;
        let budget_name = "guardrail";
        seed_budget(&router, &account_id, budget_name).await;
        let action_id = seed_pending_action(
            &router,
            &account_id,
            budget_name,
            "APPLY_IAM_POLICY",
            &iam_definition(row.identities, &policy),
        )
        .await;

        let (status, body) = execute_action(
            &router,
            &account_id,
            budget_name,
            &action_id,
            EXECUTION_TYPE_APPROVE,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the approval to answer 200, got {status} {body}"
        );
        let in_progress = read_status(&router, &account_id, budget_name, &action_id).await;
        assert_eq!(
            in_progress, ACTION_STATUS_EXECUTION_IN_PROGRESS,
            "case {case:?}: expected the action to be executing, got {in_progress:?}"
        );

        for identity in row.identities {
            let held = attached(&iam, *identity).await;
            let expected: Vec<String> = if identity.attached {
                vec![policy.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(
                held,
                expected,
                "case {case:?}: expected {:?} {} to carry {expected:?} after the execution, got {held:?}",
                identity.kind,
                identity.name
            );
        }
        let bystander = attached(&iam, role(BYSTANDER_ROLE)).await;
        assert!(
            bystander.is_empty(),
            "case {case:?}: expected the role the definition does not name to carry nothing, \
             got {bystander:?}"
        );

        make_settle_due(&engine, &account_id, budget_name, &action_id);
        let settled = read_status(&router, &account_id, budget_name, &action_id).await;
        assert_eq!(
            settled, row.settled,
            "case {case:?}: expected the execution to settle at {:?}, got {settled:?}",
            row.settled
        );
    }
}

/// An execution that takes the definition back off: a reversion takes the applied policy off
/// every identity it was attached to, and so does re-arming the action — an armed action is
/// one whose definition is not in place.
#[tokio::test]
async fn reverting_or_re_arming_an_executed_action_takes_its_policy_back_off() {
    let account_id = caller_account();
    // (case, the execution type, the status it settles at)
    let reversals: [(&str, &str, &str); 2] = [
        (
            "a reversion",
            EXECUTION_TYPE_REVERSE,
            ACTION_STATUS_REVERSE_SUCCESS,
        ),
        (
            "a reset",
            EXECUTION_TYPE_RESET,
            ACTION_STATUS_STANDBY,
        ),
    ];
    // The identity sets the rule is carried over: one of each kind, and all three at once.
    let sets: [(&str, &[Identity]); 4] = [
        ("one role", ONE_ROLE),
        ("one group", ONE_GROUP),
        ("one user", ONE_USER),
        ("roles, groups and users", ROLE_GROUP_USER),
    ];
    for (reversal, execution_type, settles_to) in reversals {
        for (identities_case, identities) in sets {
            let case = format!("{reversal} of an action attaching to {identities_case}");
            let Emulator {
                router,
                engine,
                iam,
            } = emulator();
            create_execution_role(&iam, &account_id).await;
            create_identities(&iam, identities).await;
            let policy = policy_arn(
                &iam,
                Policy::Account {
                    name: "DenyExpensiveLaunches",
                    created: true,
                },
            )
            .await;
            let budget_name = "guardrail";
            seed_budget(&router, &account_id, budget_name).await;
            let action_id = seed_pending_action(
                &router,
                &account_id,
                budget_name,
                "APPLY_IAM_POLICY",
                &iam_definition(identities, &policy),
            )
            .await;

            let (status, body) = execute_action(
                &router,
                &account_id,
                budget_name,
                &action_id,
                EXECUTION_TYPE_APPROVE,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the approval to answer 200, got {status} {body}"
            );
            make_settle_due(&engine, &account_id, budget_name, &action_id);
            let executed = read_status(&router, &account_id, budget_name, &action_id).await;
            assert_eq!(
                executed, ACTION_STATUS_EXECUTION_SUCCESS,
                "case {case:?}: expected the approval to settle at EXECUTION_SUCCESS, \
                 got {executed:?}"
            );
            for identity in identities {
                let held = attached(&iam, *identity).await;
                assert_eq!(
                    held,
                    vec![policy.clone()],
                    "case {case:?}: expected {:?} {} to carry the policy before the reversal, \
                     got {held:?}",
                    identity.kind,
                    identity.name
                );
            }

            let (status, body) = execute_action(
                &router,
                &account_id,
                budget_name,
                &action_id,
                execution_type,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {execution_type} to answer 200, got {status} {body}"
            );
            for identity in identities {
                let held = attached(&iam, *identity).await;
                assert!(
                    held.is_empty(),
                    "case {case:?}: expected {:?} {} to carry nothing after {execution_type}, \
                     got {held:?}",
                    identity.kind,
                    identity.name
                );
            }
            make_settle_due(&engine, &account_id, budget_name, &action_id);
            let settled = read_status(&router, &account_id, budget_name, &action_id).await;
            assert_eq!(
                settled, settles_to,
                "case {case:?}: expected {execution_type} to settle at {settles_to:?}, \
                 got {settled:?}"
            );
        }
    }
}

/// The same rule on the action type whose work SYSTEMS MANAGER owns: a `RUN_SSM_DOCUMENTS`
/// action runs the document its `ActionSubType` names, in the Region the definition names,
/// through Systems Manager itself.
///
/// What this asserts is the EMULATOR's refusal path, not the outcome AWS produces: on AWS
/// such an approval stops the instances and settles at `EXECUTION_SUCCESS`, while this
/// emulator's Systems Manager publishes no `AWS-Stop…` Automation runbook for either sub type
/// (`crate::state::PUBLISHED_DOCUMENTS` of the Systems Manager crate), so it answers that the
/// document does not exist. Nothing stopped any instance, so the honest report is the
/// failure, and when that gap in Systems Manager closes the same action settles at
/// `EXECUTION_SUCCESS` with nothing in this crate changing.
#[tokio::test]
async fn an_action_whose_document_systems_manager_does_not_hold_does_not_complete() {
    let account_id = caller_account();
    // The whole modelled `ActionSubType` catalog, each with the instance ids its documented
    // `InstanceIds` member carries ("the EC2 and RDS instance IDs") and a Region to run in.
    let rows: [(&str, &str, &[&str]); 3] = [
        (
            "stopping one EC2 instance",
            "STOP_EC2_INSTANCES",
            &["i-1234567890abcdef0"],
        ),
        (
            "stopping two EC2 instances in another Region",
            "STOP_EC2_INSTANCES",
            &["i-1234567890abcdef0", "i-0fedcba0987654321"],
        ),
        (
            "stopping one RDS instance",
            "STOP_RDS_INSTANCES",
            &["i-abcdef1234567890f"],
        ),
    ];
    for (case, sub_type, instance_ids) in rows {
        let (router, engine) = ssm_emulator();
        let region = if case.contains("another Region") {
            "eu-west-2"
        } else {
            REGION
        };
        let budget_name = "guardrail";
        seed_budget(&router, &account_id, budget_name).await;
        let action_id = seed_pending_action(
            &router,
            &account_id,
            budget_name,
            "RUN_SSM_DOCUMENTS",
            &json!({
                "SsmActionDefinition": {
                    "ActionSubType": sub_type,
                    "Region": region,
                    "InstanceIds": instance_ids
                }
            }),
        )
        .await;

        let (status, body) = execute_action(
            &router,
            &account_id,
            budget_name,
            &action_id,
            EXECUTION_TYPE_APPROVE,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the approval to answer 200, got {status} {body}"
        );
        make_settle_due(&engine, &account_id, budget_name, &action_id);
        let settled = read_status(&router, &account_id, budget_name, &action_id).await;
        assert_eq!(
            settled, ACTION_STATUS_EXECUTION_FAILURE,
            "case {case:?}: expected the execution to settle at EXECUTION_FAILURE, \
             got {settled:?}"
        );
    }
}

/// The same rule on the action type whose work AWS ORGANIZATIONS owns: an `APPLY_SCP_POLICY`
/// action attaches the service control policy its `PolicyId` names to every `TargetId` it
/// names, through Organizations' own `AttachPolicy`.
///
/// As with the Systems Manager table above, what this asserts is the EMULATOR's refusal path
/// rather than the outcome AWS produces: on AWS such an approval attaches the policy and
/// settles at `EXECUTION_SUCCESS`, while this emulator's Organizations answers `AttachPolicy`
/// with the 501 of an operation nobody has implemented yet. No policy is attached to any
/// target, so the action must not report that one is — and when that gap in Organizations
/// closes the same action settles at `EXECUTION_SUCCESS` with nothing in this crate changing.
#[tokio::test]
async fn an_action_whose_policy_organizations_cannot_attach_does_not_complete() {
    let account_id = caller_account();
    // Rows differ along the forms `ScpActionDefinition$TargetIds` admits — the two
    // alternatives of its documented pattern, `(ou-[0-9a-z]{4,32}-[a-z0-9]{8,32})` and
    // `\d{12}`, at the short bound of the organizational-unit form and at a longer one — and
    // along how many targets one definition names.
    let rows: [(&str, &str, &[&str]); 4] = [
        ("one account target", "p_abcd1234ef", &["111122223333"]),
        (
            "one organizational-unit target",
            "p_ou12345678",
            &["ou-abcd-1234abcd"],
        ),
        (
            "an organizational unit with longer labels",
            "p_oulong1234",
            &["ou-abcdefgh1234-1234abcdefgh5678"],
        ),
        (
            "an account and an organizational unit",
            "p_mixed12345",
            &["111122223333", "ou-abcd-1234abcd"],
        ),
    ];
    for (case, policy_suffix, target_ids) in rows {
        let (router, engine) = organizations_emulator();
        let budget_name = "guardrail";
        seed_budget(&router, &account_id, budget_name).await;
        // `PolicyId` is `^p-[0-9a-zA-Z_]{8,128}$`, so the row's suffix follows the `p-`.
        let policy_id = format!("p-{policy_suffix}");
        let action_id = seed_pending_action(
            &router,
            &account_id,
            budget_name,
            "APPLY_SCP_POLICY",
            &json!({
                "ScpActionDefinition": {
                    "PolicyId": policy_id,
                    "TargetIds": target_ids
                }
            }),
        )
        .await;

        let (status, body) = execute_action(
            &router,
            &account_id,
            budget_name,
            &action_id,
            EXECUTION_TYPE_APPROVE,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the approval to answer 200, got {status} {body}"
        );
        make_settle_due(&engine, &account_id, budget_name, &action_id);
        let settled = read_status(&router, &account_id, budget_name, &action_id).await;
        assert_eq!(
            settled, ACTION_STATUS_EXECUTION_FAILURE,
            "case {case:?}: expected the execution to settle at EXECUTION_FAILURE, \
             got {settled:?}"
        );
    }
}
