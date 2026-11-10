// SPDX-License-Identifier: CC0-1.0

//! The identity-based authorization gate this service's server, user, SSH-key and
//! access-creating operations consult, and the one place an IAM policy document is
//! evaluated.
//!
//! The remaining implemented operations (the connector, profile, workflow, web-app,
//! host-key and security-policy families) do NOT consult it yet: the gate is one
//! shared helper and pointing them at it is a mechanical follow-up, so this header
//! states the scope the code actually has rather than the scope it should grow to.
//!
//! Three questions are answered here, all of them from PERSISTED state — the
//! account's IAM store in the shared engine — and never from the request alone:
//!
//! 1. [`explicit_deny`]: does the calling principal's identity policy explicitly
//!    deny the action this request performs? An explicit `Deny` is the one IAM
//!    decision that is final whatever else the account holds, so it is the decision
//!    this emulator can make soundly: the rest of a principal's permission set
//!    (group policies, permissions boundaries, SCPs) is not modelled by the IAM
//!    emulator, so an ABSENT `Allow` here is not evidence that the caller lacks the
//!    permission and is therefore not refused. (The IAM store holds no
//!    user-attached managed policies at all, so for a user there is no further
//!    document to read; a ROLE's attachments are read, in `invocation_role_can_invoke`.)
//! 2. [`invocation_role_can_invoke`]: can the role an `API_GATEWAY` identity
//!    provider names invoke the API it names? That is the precondition
//!    `CreateServer` verifies synchronously.
//! 3. [`cross_account_role`]: does a role ARN the request asks this service to pass
//!    belong to another account? Passing a role across accounts is refused.
//!
//! Nothing here reaches outside the emulator: the policies evaluated are the ones
//! the account's own IAM emulator persisted (`PutUserPolicy`, `PutRolePolicy`,
//! `AttachRolePolicy`), and the decision is recomputed per request, so revoking a
//! policy changes the next answer.
//!
//! This module lives beside this service's state because that is where a service
//! crate may add code, and it holds only the IAM-STORE LOOKUPS that are specific to
//! this gate: which documents are in force on the caller, and which on an invocation
//! role. The DECISION those documents imply is not made here — it is
//! [`::service_common::iam_authorization`], the one shared evaluation of IAM policy
//! documents against `(action, resource)`, so this service and every other one handed
//! the same documents answer alike (CloudWatch RUM's gate consumes the same module).
//! That is also where IAM's reading of a `Condition` this emulator cannot decide
//! lives: such a statement never grants and still refuses.

use ::service_common::iam_authorization::{self as iam_policy, Decision};

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

/// This service's IAM action prefix: an action is `transfer:<OperationName>`
/// ("Actions defined by Amazon Transfer Family", IAM Service Authorization
/// Reference).
pub const ACTION_PREFIX: &str = "transfer";

/// The action an `API_GATEWAY` identity provider's `InvocationRole` has to be
/// allowed to perform on the API: `IdentityProviderDetails$InvocationRole` is "the
/// type of `InvocationRole` used to authenticate the user account", and an API
/// Gateway REST endpoint is invoked with `execute-api:Invoke`.
const EXECUTE_API_INVOKE: &str = "execute-api:Invoke";

/// The `Resource` a request that addresses NO nameable ARN is decided against.
///
/// A listing, and a create whose identifier this service mints, name no resource to
/// compare a statement's `Resource` against. The empty value is what expresses that
/// in the shared evaluator's terms: IAM's wildcard matching lets only a pattern that
/// covers every resource (`"*"`) match it, so a narrower statement — one scoped to a
/// server or a user — cannot decide such a request, while a statement that states its
/// resources as `NotResource` still covers it.
const NO_RESOURCE: &str = "";

/// The account's IAM store in the shared engine.
///
/// The IAM service's OWN accessor is called rather than copied, so this gate reads
/// the exact store IAM's handlers write. `None` when the emulator is not the
/// in-memory engine, which is EVIDENCE OF NOTHING, and so refuses nothing in either
/// caller: [`explicit_deny`] finds no stored deny, and
/// [`invocation_role_can_invoke`] cannot show the role unable to invoke.
fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<iam::ComAmazonawsIamAwsidentitymanagementv20100508State>> {
    iam::service_state(engine)
}

/// The refusal text IAM states an explicit-deny decision with: who was refused,
/// which action, ON WHICH RESOURCE the action was evaluated, and that the refusal came
/// from an explicit deny in an identity-based policy. Every value in it comes from the
/// request and the stored policy, not from a recording.
///
/// The resource clause names the ARN the decision was taken against — the same value
/// [`explicit_deny`] evaluated the action on — so a caller parsing the sentence can
/// attribute the denial. A request that addresses no nameable ARN has none to state and
/// is told the principal and the action alone ([`NO_RESOURCE`] is what it was decided
/// against).
fn not_authorized(principal: &str, action: &str, resource: Option<&str>) -> String {
    match resource {
        Some(resource) => format!(
            "User: {principal} is not authorized to perform: {action} \
             on resource: {resource} \
             with an explicit deny in an identity-based policy"
        ),
        None => format!(
            "User: {principal} is not authorized to perform: {action} \
             with an explicit deny in an identity-based policy"
        ),
    }
}

/// The refusal text for a role ARN in another account, as the service states it.
pub const CROSS_ACCOUNT_PASS_ROLE: &str = "Cross-account pass role is not allowed.";

/// `Some(message)` when the caller's identity policy explicitly denies
/// `transfer:<operation>` on `resource` — the message a modelled
/// `AccessDeniedException` carries — and `None` when no stored policy denies it.
///
/// `resource` is the ARN the request addresses — including the ARN of a resource the
/// request would CREATE, when the request itself names it (a user is addressed by
/// its server and name, so `CreateUser` passes it) — or `None` for a request that
/// addresses no nameable resource: a listing, and a create whose identifier this
/// service mints (`CreateServer`). See [`NO_RESOURCE`] for what `None` means to the
/// shared evaluator.
pub fn explicit_deny<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &::http::request::Parts,
    operation: &str,
    resource: Option<&str>,
) -> Option<String> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let state = iam_state(engine)?;
    let guard = state.lock();
    // The calling principal is the one that signed: the access key id in the
    // SigV4 credential, resolved through the keys IAM minted. A key IAM never
    // minted belongs to no IAM user — the emulator's own root credential, say —
    // and the account root is subject to no identity policy.
    let record = guard.access_keys.get(&::engine::extract_akid(&parts.headers))?;
    let principal = guard
        .users
        .get(&record.user_key)
        .map(|user| user.arn.clone())
        .unwrap_or_else(|| record.user_key.clone());
    // The documents in force on the caller: its inline policies. The IAM store holds
    // no user-attached managed policies at all (it has `user_inline_policies` and
    // `role_attached_policies`), so there is no further document to read for a user.
    let documents: Vec<String> = guard
        .user_inline_policies
        .get(&record.user_key)
        .into_iter()
        .flat_map(|policies| policies.values())
        .map(|policy| policy.policy_document.clone())
        .collect();
    let request = iam_policy::Request::new(&action, resource.unwrap_or(NO_RESOURCE), &[]);
    (iam_policy::identity_decision(&documents, &request) == Decision::Denied)
        .then(|| not_authorized(&principal, &action, resource))
}

/// The IAM principal this request's credential speaks for, as a refusal names it.
///
/// The calling principal is the one that SIGNED: the access key id in the SigV4
/// credential, resolved through the keys IAM minted, exactly as [`explicit_deny`]
/// resolves it. A key IAM never minted belongs to no IAM user of the account's store —
/// the emulator's own long-term credential — and that credential's principal is the one
/// the account contract states for it
/// ([`::service_common::caller_principal::ambient_user_arn`]), which is also the
/// principal STS reports for it, so a client that reads its own identity sees the same
/// value a refusal names it by.
pub fn calling_principal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &::http::request::Parts,
) -> String {
    let ambient = ::service_common::caller_principal::ambient_user_arn;
    let Some(state) = iam_state(engine) else {
        return ambient();
    };
    let guard = state.lock();
    guard
        .access_keys
        .get(&::engine::extract_akid(&parts.headers))
        .and_then(|record| guard.users.get(&record.user_key))
        .map(|user| user.arn.clone())
        .unwrap_or_else(ambient)
}

/// Whether the role `role_arn` names can invoke the API `url` names, according to
/// the role's own stored identity policies.
///
/// IAM's deny-overrides rule decides: an explicit `Deny` of the invoke in any of the
/// role's readable policies means it cannot invoke, whatever else allows it, and
/// otherwise an `Allow` that covers the API is what makes the verification succeed.
/// A policy attached to the role that this account's store does not hold — an
/// AWS-managed one, which AWS owns — can neither be read nor be ruled out, and this
/// service refuses only what it can SHOW to be unverifiable, so such a role passes.
///
/// A role this account does not hold cannot invoke anything, so an ARN naming no
/// stored role answers `false`: the verification FAILED, which is the same answer
/// the real service gives for a role it cannot assume or that cannot reach the API.
/// A store that cannot be read at all, by contrast, shows nothing and refuses
/// nothing.
/// A `url` that names no API Gateway invoke endpoint leaves no API ARN to compare, so
/// only a policy covering every resource can verify such a provider.
///
/// `region` is the Region the calling request was signed for, which is the Region an
/// invoke endpoint that does not carry one in its host is addressed in — see
/// [`execute_api_arn`].
pub fn invocation_role_can_invoke<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
    url: Option<&str>,
    region: &str,
) -> bool {
    if cross_account_role(role_arn) {
        return false;
    }
    // No store to read is EVIDENCE OF NOTHING, exactly like an attached policy this
    // account does not own (see `undecidable` below): it cannot show the role unable
    // to invoke, so it does not refuse. This is the same reading `explicit_deny`
    // takes of a missing store.
    let Some(state) = iam_state(engine) else {
        return true;
    };
    let Some(role_key) = role_name(role_arn).map(iam::name_key) else {
        return false;
    };
    let guard = state.lock();
    if !guard.roles.contains_key(&role_key) {
        return false;
    }
    // The API the URL names, as an `execute-api` ARN, so a policy scoped to one
    // API is honoured; `None` when the URL names no API Gateway endpoint, which
    // leaves only a statement covering every resource able to allow the invoke.
    let api = url.and_then(|url| execute_api_arn(url, region));
    let inline = guard
        .role_inline_policies
        .get(&role_key)
        .into_iter()
        .flat_map(|policies| policies.values())
        .map(|policy| policy.policy_document.clone());
    // The policies attached to the role, and whether ANY of them is one this account
    // does not own: an AWS-MANAGED policy, whose document AWS holds rather than this
    // account (`AmazonAPIGatewayInvokeFullAccess` is AWS's), so it can neither be read
    // nor be ruled out and is evidence of nothing. An attachment naming a
    // CUSTOMER-managed policy the account does not hold is not that case — the account
    // IS the authority on its own policies, so such an attachment grants nothing —
    // which is the same reading CloudWatch RUM's gate takes of the same store.
    let mut undecidable = false;
    let mut documents: Vec<String> = inline.collect();
    for arn in guard
        .role_attached_policies
        .get(&role_key)
        .into_iter()
        .flatten()
    {
        match default_policy_document(&guard, arn) {
            Some(document) => documents.push(document),
            None if is_aws_managed_policy(arn) => undecidable = true,
            None => {}
        }
    }

    // The decision those documents imply, from the ONE shared evaluator: a `Deny`
    // about this invoke wins over every `Allow`, an `Allow` that covers the API is
    // what verifies the role, and documents that say neither decide nothing.
    let request = iam_policy::Request::new(
        EXECUTE_API_INVOKE,
        api.as_deref().unwrap_or(NO_RESOURCE),
        &[],
    );
    match iam_policy::identity_decision(&documents, &request) {
        Decision::Denied => false,
        Decision::Allowed => true,
        // An AWS-managed policy attached to the role might be the one that allows the
        // invoke, and this service refuses only what it can SHOW to be unverifiable —
        // the same principle `explicit_deny` applies to a permission set this emulator
        // does not model in full.
        Decision::NotStated => undecidable,
    }
}

/// The vendor field of an ARN that names an IAM resource, and the resource type of
/// one that names a role: `arn:<partition>:iam::<account>:role/<path>/<name>`.
const IAM_VENDOR: &str = "iam";
const ROLE_RESOURCE_TYPE: &str = "role";

/// Whether `role_arn` names a role in an account other than this one.
///
/// The ARN's fields are read through the ONE shared parse
/// ([`service_common::arn::parse`]) rather than split again here. An ARN whose
/// account field is empty names no account to compare, so it is not a cross-account
/// ARN, and neither is a value that is no ARN at all — that shape is already refused
/// by the member's modelled pattern.
pub fn cross_account_role(role_arn: &str) -> bool {
    ::service_common::arn::parse(role_arn)
        .map(|arn| arn.account)
        .filter(|account| !account.is_empty())
        .is_some_and(|account| account != ::engine::emulator_account_id())
}

/// The role name an IAM role ARN names: the last segment of the resource field of
/// `arn:<partition>:iam::<account>:role/<path>/<name>`.
///
/// `None` for a value that is not a well-formed ARN, that names another vendor's
/// resource, or whose resource field is not a `role/…` — none of those names a role
/// this account's IAM store could hold.
fn role_name(role_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse(role_arn)?;
    if arn.vendor != IAM_VENDOR || arn.resource_type() != ROLE_RESOURCE_TYPE {
        return None;
    }
    let (_, path_and_name) = arn.resource.split_once('/')?;
    path_and_name
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
}

/// Whether `arn` names an AWS-MANAGED policy: one whose account segment is IAM's own
/// `aws`, so AWS owns the document and this account's store holds none for it. Read
/// through IAM's OWN ARN parser and its own constant, so this service and IAM agree
/// on what an AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam::parse_policy_arn(arn).is_some_and(|policy| policy.account == iam::AWS_MANAGED_POLICY_ACCOUNT)
}

/// The default version's document of the managed policy `arn` names, or `None` when
/// the account holds no such policy or no document for its default version.
///
/// The version is the one `Policy$DefaultVersionId` names, falling back to the version
/// that carries `PolicyVersion$IsDefaultVersion` — the two spellings of "the version in
/// force" the IAM store can hold.
fn default_policy_document(
    iam_state: &iam::ComAmazonawsIamAwsidentitymanagementv20100508State,
    arn: &str,
) -> Option<String> {
    let managed = iam_state.policy_by_arn(arn)?;
    managed
        .policy
        .default_version_id
        .as_deref()
        .and_then(|default| managed.versions.get(default))
        .or_else(|| {
            managed
                .versions
                .values()
                .find(|version| version.is_default_version)
        })
        .and_then(|version| version.document.clone())
}

/// The vendor of an API Gateway invoke endpoint, which is also the label that names
/// that data plane in both addressings below: `execute-api`.
const EXECUTE_API: &str = "execute-api";

/// The `execute-api` ARN of the API an API Gateway invoke URL addresses, in EITHER
/// addressing a deployed stage is reached at.
///
/// A stage invoke endpoint names two things the policy resource needs — WHICH api, and
/// that the address is the `execute-api` data plane — and where it carries them depends
/// on the host the caller can resolve:
///
/// 1. `https://<api-id>.execute-api.<region>.amazonaws.com/<stage>/…` carries both in
///    the HOST, with the Region beside them;
/// 2. `https://<endpoint>/_aws/execute-api/<api-id>/<stage>/…` carries both in the PATH,
///    behind the reserved namespace segment, because a caller reaching one endpoint for
///    every service and every plane cannot carry them in the host. That is the
///    addressing this emulator PUBLISHES as a deployed stage's invoke URL, so it is an
///    invoke endpoint exactly as the first is — and the segments are read through the
///    shared reading of that namespace, not a second copy of it.
///
/// Either way the resource a policy names for the API is
/// `arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/*`. The second form carries
/// no Region, so the API is read as being in `region` — the Region the calling request
/// was signed for, which is the one the caller's own API Gateway calls were signed for
/// too. `None` for a URL that is not an invoke endpoint in either form, so nothing is
/// derived from a URL that names no API.
fn execute_api_arn(url: &str, region: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = ::service_common::data_plane_addressing::host_only(authority);
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());

    // The host form: the api id is the first label and `execute-api` the second, with
    // the Region after it; the stage is the first path segment.
    if let Some((api_id, domain)) = host.split_once('.') {
        let mut labels = domain.split('.');
        if labels.next() == Some(EXECUTE_API) {
            let region = labels.next()?;
            let stage = segments.next()?;
            return Some(api_arn(region, api_id, stage));
        }
    }

    // The path form: the reserved namespace segment, the plane, then the api id and the
    // stage.
    if segments.next()? != ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
        || segments.next()? != EXECUTE_API
    {
        return None;
    }
    let api_id = segments.next()?;
    let stage = segments.next()?;
    Some(api_arn(region, api_id, stage))
}

/// `arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/*` — the resource an IAM
/// policy names to allow invoking one stage of one API.
fn api_arn(region: &str, api_id: &str, stage: &str) -> String {
    ::engine::emulator_arn(EXECUTE_API, region, &format!("{api_id}/{stage}/*"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults — the decision both public functions above read.
    fn decision(document: &str, action: &str, resource: Option<&str>) -> Decision {
        iam_policy::identity_decision(
            &[document.to_string()],
            &iam_policy::Request::new(action, resource.unwrap_or(NO_RESOURCE), &[]),
        )
    }

    /// Whether one stored document explicitly denies `action` on `resource` — the
    /// question [`explicit_deny`] asks of the caller's policies.
    fn denies(document: &str, action: &str, resource: Option<&str>) -> bool {
        decision(document, action, resource) == Decision::Denied
    }

    const DENY_CREATE_SERVER: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "transfer:*", "Resource": "*"},
            {"Effect": "Deny", "Action": "transfer:CreateServer", "Resource": "*"}
        ]
    }"#;

    #[test]
    fn an_explicit_deny_is_found_for_the_action_it_names_only() {
        assert!(
            denies(DENY_CREATE_SERVER, "transfer:CreateServer", None),
            "the denied action must be found"
        );
        assert!(
            !denies(DENY_CREATE_SERVER, "transfer:CreateUser", None),
            "an action the deny does not name must not be denied"
        );
        assert_eq!(
            decision(DENY_CREATE_SERVER, "transfer:CreateUser", None),
            Decision::Allowed,
            "the allow the document also states must still be read"
        );
    }

    #[test]
    fn a_wildcard_action_matches_case_insensitively() {
        let document = r#"{"Statement": {"Effect": "Deny", "Action": "TRANSFER:Create*", "Resource": "*"}}"#;
        assert!(
            denies(document, "transfer:CreateUser", None),
            "a wildcard action must match case-insensitively"
        );
    }

    #[test]
    fn a_statement_whose_condition_cannot_be_decided_still_refuses() {
        // The shared evaluator's fail-closed reading, which this gate inherits: a
        // `Deny` whose condition MAY hold is never weakened into permission, while an
        // `Allow` whose condition may not hold is no grant.
        let denied = r#"{"Statement": [{"Effect": "Deny", "Action": "transfer:*",
            "Resource": "*", "Condition": {"Bool": {"aws:SecureTransport": "false"}}}]}"#;
        assert!(
            denies(denied, "transfer:CreateServer", None),
            "a deny this emulator cannot decide must still refuse"
        );
        let allowed = r#"{"Statement": [{"Effect": "Allow", "Action": "transfer:*",
            "Resource": "*", "Condition": {"Bool": {"aws:SecureTransport": "false"}}}]}"#;
        assert_eq!(
            decision(allowed, "transfer:CreateServer", None),
            Decision::NotStated,
            "an allow this emulator cannot decide must not grant"
        );
    }

    #[test]
    fn a_malformed_document_denies_nothing() {
        assert!(
            !denies("{not json", "transfer:CreateServer", None),
            "a document that does not parse carries no statement"
        );
    }

    #[test]
    fn a_resource_scoped_deny_does_not_cover_an_unnamed_resource() {
        let document = r#"{"Statement": {"Effect": "Deny", "Action": "transfer:DeleteServer",
            "Resource": "arn:aws:transfer:us-east-1:000000000000:server/s-1"}}"#;
        assert!(
            denies(
                document,
                "transfer:DeleteServer",
                Some("arn:aws:transfer:us-east-1:000000000000:server/s-1"),
            ),
            "the named resource must be denied"
        );
        assert!(
            !denies(
                document,
                "transfer:DeleteServer",
                Some("arn:aws:transfer:us-east-1:000000000000:server/s-2"),
            ),
            "another resource must not be denied"
        );
        assert!(
            !denies(document, "transfer:DeleteServer", None),
            "a request that names no resource must not be covered by it"
        );
    }

    #[test]
    fn an_invoke_url_resolves_to_the_apis_execute_api_arn() {
        // The rows span both addressings a deployed stage is reached at (the host form
        // AWS publishes and the path form this emulator publishes), the parts each one
        // carries (api id, Region, stage, a resource path after the stage, a port on the
        // endpoint), and values that are no invoke endpoint at all. `request_region` is
        // the Region the calling request was signed for, which only the path form
        // — carrying none of its own — is read in.
        let request_region = "us-west-2";
        // (case, url, the execute-api resource it names)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "the host form AWS publishes",
                "https://abc123.execute-api.us-east-1.amazonaws.com/prod",
                Some("arn:aws:execute-api:us-east-1:000000000000:abc123/prod/*"),
            ),
            (
                "the host form with a resource path after the stage",
                "https://abc123.execute-api.eu-west-1.amazonaws.com/test/auth",
                Some("arn:aws:execute-api:eu-west-1:000000000000:abc123/test/*"),
            ),
            (
                "the host form naming no stage",
                "https://abc123.execute-api.us-east-1.amazonaws.com/",
                None,
            ),
            (
                "the path form this emulator publishes, on a loopback endpoint",
                "https://127.0.0.1:39477/_aws/execute-api/8sh3tqaaaa/sim2real",
                Some("arn:aws:execute-api:us-west-2:000000000000:8sh3tqaaaa/sim2real/*"),
            ),
            (
                "the path form with a resource path after the stage",
                "https://localhost:9297/_aws/execute-api/8sh3tqaaaa/test/auth",
                Some("arn:aws:execute-api:us-west-2:000000000000:8sh3tqaaaa/test/*"),
            ),
            (
                "the path form over plain HTTP",
                "http://127.0.0.1:9297/_aws/execute-api/zzz999/prod",
                Some("arn:aws:execute-api:us-west-2:000000000000:zzz999/prod/*"),
            ),
            (
                "the path form naming no stage",
                "https://127.0.0.1:9297/_aws/execute-api/zzz999",
                None,
            ),
            (
                "a path that names another plane under the same namespace",
                "https://127.0.0.1:9297/_aws/portal/zzz999/prod",
                None,
            ),
            (
                "a path whose namespace segment merely starts with the reserved one",
                "https://127.0.0.1:9297/_awsx/execute-api/zzz999/prod",
                None,
            ),
            ("a URL that is not an invoke endpoint", "https://example.invalid/auth", None),
            ("a value that is no URL at all", "the-api", None),
        ];
        for (case, url, expected) in rows {
            let resolved = execute_api_arn(url, request_region);
            assert_eq!(
                resolved.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    #[test]
    fn an_explicit_deny_names_the_resource_it_was_decided_against() {
        let principal = "arn:aws:iam::000000000000:user/denied";
        let resource = "arn:aws:transfer:us-west-2:000000000000:server/*";
        assert_eq!(
            not_authorized(principal, "transfer:CreateServer", Some(resource)),
            format!(
                "User: {principal} is not authorized to perform: transfer:CreateServer \
                 on resource: {resource} with an explicit deny in an identity-based policy"
            ),
            "the denial must name the principal, the action and the resource"
        );
        assert_eq!(
            not_authorized(principal, "transfer:ListServers", None),
            format!(
                "User: {principal} is not authorized to perform: transfer:ListServers \
                 with an explicit deny in an identity-based policy"
            ),
            "a request that addresses no nameable resource states none"
        );
    }

    #[test]
    fn a_role_arn_in_another_account_is_cross_account() {
        assert!(
            cross_account_role("arn:aws:iam::111122223333:role/idp"),
            "another account's role is cross-account"
        );
        assert!(
            !cross_account_role("arn:aws:iam::000000000000:role/idp"),
            "this account's role is not cross-account"
        );
        assert!(
            !cross_account_role("not-an-arn-at-all"),
            "a value that is no ARN names no account to compare"
        );
    }

    #[test]
    fn the_role_name_is_the_last_segment_of_the_arn() {
        // (case, value, the role it names)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a path'd role ARN names the role by its last segment",
                "arn:aws:iam::000000000000:role/service-role/transfer-idp",
                Some("transfer-idp"),
            ),
            (
                "a role ARN with no path",
                "arn:aws:iam::000000000000:role/transfer-idp",
                Some("transfer-idp"),
            ),
            (
                "another vendor's resource names no IAM role",
                "arn:aws:transfer:us-east-1:000000000000:role/transfer-idp",
                None,
            ),
            (
                "an IAM resource that is not a role",
                "arn:aws:iam::000000000000:user/transfer-idp",
                None,
            ),
            (
                "a resource field that is the bare type",
                "arn:aws:iam::000000000000:role",
                None,
            ),
            ("a value that is no ARN", "transfer-idp", None),
        ];
        for (case, value, expected) in rows {
            assert_eq!(
                role_name(value),
                *expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                role_name(value)
            );
        }
    }
}
