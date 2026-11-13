// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER's own identity-based IAM policies let it run a query on a graph's
//! data plane — the decision `ExecuteQuery` takes before it reads the graph.
//!
//! Running a query is the one thing this service does on behalf of a caller's own IAM
//! principal rather than on a resource the control plane addresses: "the IAM user or
//! role making the request must have a policy attached that allows one of the following
//! IAM actions in that cluster, depending on the query: `neptune-graph:ReadDataViaQuery`
//! `neptune-graph:WriteDataViaQuery` `neptune-graph:DeleteDataViaQuery`"
//! (`smithy_query(query="doc",shape="ExecuteQuery")`). So a caller whose policies grant
//! none of them is refused with the service's denial shape whatever the query says, and
//! the data plane never reads the graph for it — which is what makes a MISSING GRANT
//! distinguishable from a graph that is not serving (reported as the resource it is).
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, resolved by this
//!    service's one reading of that question ([`crate::state::caller_principal`]), so
//!    the principal a denial blames is the one `sts:GetCallerIdentity` reports for the
//!    same credentials;
//! 2. **what it may do** — the identity-based documents in force on that principal and
//!    the permissions boundary that caps them, read through the IAM service's OWN
//!    accessors, so a policy `iam:PutUserPolicy` wrote takes effect on the next query
//!    and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared
//!    evaluation of those documents against `(action, resource)`.
//!
//! Only the store LOOKUPS live here; neither the collection nor the evaluation does.
//!
//! ## The resource and the actions
//!
//! The resource is the GRAPH the request named, as an ARN of this service's namespace,
//! built from the identifier the request carried rather than from a resolved record: the
//! decision is then the same whether or not that identifier names anything, which is
//! what keeps a refusal from telling an unauthorized caller what the account holds.
//!
//! The actions are the three the operation's own documentation names, and ONE of them
//! suffices, as that sentence states. A caller that holds none is told which three were
//! asked about, because a client fixing its policy needs them all.
//!
//! ## The two callers the decision cannot refuse
//!
//! * **The emulator's own published credential.** A request whose access key id neither
//!   STS nor IAM holds a record for speaks for the ambient principal, which carries no
//!   policy of the account's: the position AWS account root credentials are in. Such a
//!   request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to the account, so the IAM store holds no
//!   document for it; refusing on a grant this emulator cannot see would deny access AWS
//!   permits. An explicit `Deny` in a document that CAN be read still refuses, and a
//!   permissions boundary the store DOES hold still caps the grant.

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// The IAM actions a query is served under, in the order a refusal names them: the three
/// the operation's own documentation lists (see the module docs). One of them suffices.
pub const QUERY_ACTIONS: &[&str] = &[
    "neptune-graph:DeleteDataViaQuery",
    "neptune-graph:ReadDataViaQuery",
    "neptune-graph:WriteDataViaQuery",
];

/// The ARN of the graph a data-plane request addresses, in the Region it was signed for.
///
/// Built through the shared ARN builder every ARN this emulator emits comes from, under
/// this service's own namespace — the same spelling a stored graph's `arn` carries — from
/// the identifier the request carried.
#[must_use]
pub fn graph_resource(region: &str, graph_identifier: &str) -> String {
    ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        region,
        &format!("graph/{graph_identifier}"),
    )
}

/// The modelled `AccessDeniedException` (HTTP 403) a caller whose identity-based policies
/// allow none of [`QUERY_ACTIONS`] on `graph_arn` is refused with, or `None` when IAM
/// allows the query.
///
/// IAM's order: an explicit `Deny` in a document that CAN be read refuses whatever else
/// the principal carries; otherwise a grant is needed, which a caller this gate does not
/// bound has by definition (see the module docs); and a grant is capped by the
/// permissions boundary, since "a permissions boundary sets the maximum permissions that
/// an identity-based policy can grant" (IAM, permissions boundaries).
///
/// The sentence is the one AWS refuses an unauthorized data-plane call of THIS service
/// with: the principal that signed, the actions it would have needed and the resource it
/// named, and nothing after it. That is why it is not built through the shared
/// [`::service_common::iam_authorization::not_authorized_by_identity_policy`] every gate
/// whose refusal ends in a reason clause reaches: that sentence closes with WHICH of
/// IAM's rules withheld the grant, and this one closes at the resource — the three
/// actions it lists are already the whole statement of what was missing, and a clause
/// the service does not send would be this emulator's own words in a recorded sentence.
/// The shared helper stays the one builder for the refusals that do carry a reason.
///
/// Asked with NO store guard of this service held, and before the graph is read.
#[must_use]
pub fn query_denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    graph_arn: &str,
) -> Option<AccessDeniedException> {
    let caller = caller(engine, ctx);
    if caller.may_query(graph_arn) {
        return None;
    }
    Some(AccessDeniedException {
        message: format!(
            "User: {} is not authorized to perform: {} on resource: {graph_arn}",
            caller.arn,
            QUERY_ACTIONS.join(", ")
        ),
    })
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the emulator's own published credential, which
    /// holds no IAM policies (see the module docs).
    ambient_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does
    /// not hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether IAM lets this caller run a query on `graph_arn`: one of
    /// [`QUERY_ACTIONS`] is allowed, and the boundary allows that same action.
    fn may_query(&self, graph_arn: &str) -> bool {
        QUERY_ACTIONS
            .iter()
            .any(|action| self.allows(action, graph_arn))
    }

    /// Whether one action is allowed: never when a readable document denies it, and
    /// otherwise when a document allows it — or when nothing bounds this caller — and the
    /// permissions boundary does not withhold it.
    fn allows(&self, action: &str, graph_arn: &str) -> bool {
        let request = iam::Request::new(action, graph_arn, &[]);
        let said = iam::identity_decision(&self.documents, &request);
        if said == Decision::Denied {
            return false;
        }
        if !(iam::is_allowed(said, Decision::NotStated) || self.identity_is_unbounded()) {
            return false;
        }
        match &self.boundary {
            None => true,
            Some(boundary) => {
                iam::identity_decision(::std::slice::from_ref(boundary), &request)
                    == Decision::Allowed
            }
        }
    }

    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the emulator's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.ambient_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// OWN accessors.
    fn read_policies_of(
        &mut self,
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on
/// it.
///
/// WHICH principal is this service's one reading of that question
/// ([`crate::state::caller_principal`]), so a denial here and a pass-role denial blame
/// the same identity for the same credentials; only the POLICIES are collected here.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let arn = crate::state::caller_principal(engine, ctx);
    let mut caller = Caller {
        ambient_credential: arn == sts_store::ambient_identity().arn,
        arn,
        ..Caller::default()
    };
    let Some(iam) = sts_store::iam_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // Which IAM identity the principal's policies are held under is read through the
    // tree's shared readings of a principal ARN.
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A caller carrying `documents` and nothing else: neither the emulator's own
    /// credential nor an unreadable attachment, which is the position an IAM user of the
    /// account is in.
    fn user_with(documents: &[&str]) -> Caller {
        Caller {
            arn: String::from("arn:aws:iam::000000000000:user/analyst"),
            documents: documents.iter().map(|text| (*text).to_string()).collect(),
            ..Caller::default()
        }
    }

    /// RULE: ONE of the three query actions suffices, and a policy that grants none of
    /// them — including one scoped to another graph, or to another service's actions —
    /// does not let the query run.
    #[test]
    fn one_query_action_suffices_and_none_refuses() {
        let graph = graph_resource("us-east-1", "g-0123456789");
        let other = graph_resource("us-east-1", "g-9876543210");
        let allow = |action: &str, resource: &str| {
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
                "Action":"{action}","Resource":"{resource}"}}]}}"#
            )
        };
        // (case, the one document the caller carries, whether the query may run)
        let rows: &[(&str, String, bool)] = &[
            (
                "read on the graph",
                allow("neptune-graph:ReadDataViaQuery", &graph),
                true,
            ),
            (
                "write on the graph",
                allow("neptune-graph:WriteDataViaQuery", &graph),
                true,
            ),
            (
                "delete on the graph",
                allow("neptune-graph:DeleteDataViaQuery", &graph),
                true,
            ),
            (
                "every action of the service on the graph",
                allow("neptune-graph:*", &graph),
                true,
            ),
            (
                "read on another graph",
                allow("neptune-graph:ReadDataViaQuery", &other),
                false,
            ),
            (
                "a control-plane action on the graph",
                allow("neptune-graph:GetGraph", &graph),
                false,
            ),
            (
                "another service's actions",
                allow("s3:GetObject", "*"),
                false,
            ),
            ("no policy at all", String::new(), false),
        ];
        for (case, document, expected) in rows {
            let caller = if document.is_empty() {
                user_with(&[])
            } else {
                user_with(&[document.as_str()])
            };
            let actual = caller.may_query(&graph);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected may_query == {expected}, got {actual}"
            );
        }
    }

    /// RULE: an explicit `Deny` is final, so a caller denied every query action cannot
    /// run a query however wide its grant is; a deny on ONE action leaves the others.
    #[test]
    fn an_explicit_deny_wins_over_a_wildcard_grant() {
        let graph = graph_resource("eu-west-1", "g-0123456789");
        let denied_all = user_with(&[r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"neptune-graph:*","Resource":"*"},
            {"Effect":"Deny","Action":["neptune-graph:ReadDataViaQuery",
             "neptune-graph:WriteDataViaQuery","neptune-graph:DeleteDataViaQuery"],
             "Resource":"*"}]}"#]);
        assert!(
            !denied_all.may_query(&graph),
            "case \"every query action denied\": expected the query to be refused"
        );
        let denied_one = user_with(&[r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"neptune-graph:*","Resource":"*"},
            {"Effect":"Deny","Action":"neptune-graph:WriteDataViaQuery","Resource":"*"}]}"#]);
        assert!(
            denied_one.may_query(&graph),
            "case \"one query action denied\": expected the other two to still allow it"
        );
    }

    /// RULE: a permissions boundary caps the grant — a caller allowed by its own policies
    /// and not by the boundary may not query.
    #[test]
    fn a_permissions_boundary_caps_the_grant() {
        let graph = graph_resource("us-west-2", "g-0123456789");
        let mut caller = user_with(&[
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
            "Action":"neptune-graph:*","Resource":"*"}]}"#,
        ]);
        caller.boundary = Some(String::from(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
            "Action":"s3:GetObject","Resource":"*"}]}"#,
        ));
        assert!(
            !caller.may_query(&graph),
            "case \"a boundary naming no query action\": expected the query to be refused"
        );
        caller.boundary = Some(String::from(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
            "Action":"neptune-graph:ReadDataViaQuery","Resource":"*"}]}"#,
        ));
        assert!(
            caller.may_query(&graph),
            "case \"a boundary naming one query action\": expected the query to run"
        );
    }

    /// RULE: the caller nothing bounds — the emulator's own published credential — runs
    /// queries without a policy, while an account identity with no policy does not.
    #[test]
    fn the_ambient_credential_is_not_refused() {
        let graph = graph_resource("us-east-1", "g-0123456789");
        let ambient = Caller {
            arn: sts_store::ambient_identity().arn,
            ambient_credential: true,
            ..Caller::default()
        };
        assert!(
            ambient.may_query(&graph),
            "case \"the emulator's own credential\": expected the query to run"
        );
        assert!(
            !user_with(&[]).may_query(&graph),
            "case \"an IAM user with no policy\": expected the query to be refused"
        );
    }
}
