// SPDX-License-Identifier: CC0-1.0

//! The portal reports the sign-in and the assignments the REST of the emulator recorded —
//! end to end over HTTP, with no portal state seeded by hand.
//!
//! RULE: a presented access token names a sign-in exactly when the emulator's own IAM
//! Identity Center OIDC service issued that token and it is still inside its own lifetime;
//! the accounts that sign-in reports, and the roles in each, are the IAM Identity Center
//! account assignments whose grant has completed — a role being the NAME of the permission
//! set the assignment grants — and an assigned account's display name and email address
//! are the organization's own records of that account.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, the one path a
//! socket-served request takes, against the four services the production router mounts on
//! one shared engine: the portal, IAM Identity Center OIDC (which mints the token), IAM
//! Identity Center (where the administrator's assignments live) and Organizations (where an
//! account's name and email live). Nothing is written into the portal's own store by the
//! test: the sign-in and the assignments arrive the way they do in a running emulator.
//!
//! Two set-up steps are not calls, because the emulator serves no API for them, and both
//! are the clock's or the user's business rather than the wire's:
//!
//! * the user's APPROVAL of the device authorization, which IAM Identity Center takes at
//!   the access-portal page and no operation of the OIDC API performs;
//! * the completion INSTANT of an assignment's asynchronous grant, moved so both sides of
//!   it can be observed without the suite sleeping — the status itself is still derived by
//!   IAM Identity Center, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_organizations_awsorganizationsv20161128 as organizations;
use service_com_amazonaws_sso_swbportalservice::state::INVALID_SESSION_MESSAGE;
use service_com_amazonaws_sso_swbportalservice::EmulatedService as PortalService;
use service_com_amazonaws_ssoadmin_swbexternalservice as identity_center;
use service_com_amazonaws_ssooidc_awsssooidcservice as oidc;

/// The portal endpoint an SDK addresses. Every portal operation is `@optionalAuth` with an
/// empty `@auth`, so the request carries no SigV4 scope and the service claims it by this
/// host.
const PORTAL_HOST: &str = "portal.sso.us-east-1.amazonaws.com";

/// The Region every signed call of this suite is scoped to, and so the Region the
/// account's IAM Identity Center instance is enabled in.
const REGION: &str = "us-east-1";

/// One account an administrator assigns permission sets on.
struct Granted {
    label: &'static str,
    /// `TargetId`, modelled `^\d{12}$`. The rows span the published bounds of that
    /// pattern (all zeros, all nines) and one id in between.
    account_id: String,
    /// The permission sets granted on the account, in the order the test creates them —
    /// NOT the order the portal must report their names in. `Name` is modelled
    /// `^[\w+=,.@-]+$`, length 1..32.
    permission_sets: &'static [&'static str],
    /// `PrincipalType`: the model's enum is `USER | GROUP`.
    principal_type: &'static str,
    /// `PrincipalId`: both forms the modelled pattern admits appear below (a bare GUID and
    /// the identity-store-prefixed form), in lower and upper case hex.
    principal_id: &'static str,
}

/// The assignments the rules below are asserted over: a different number of permission
/// sets per account, both `PrincipalType` values, an account the organization HOLDS a
/// record of and one it does not, and role names that sort differently from the order they
/// are created in.
fn granted() -> Vec<Granted> {
    vec![
        Granted {
            label: "the organization's management account, two permission sets, a USER principal",
            // The account the emulator's organization holds a record of, so its name and
            // email address are reportable.
            account_id: engine::emulator_account_id().to_string(),
            permission_sets: &["ReadOnly", "AdministratorAccess"],
            principal_type: "USER",
            principal_id: "9067f3a1c2-f81d4fae-7dec-11d0-a765-00a0c91e6bf7",
        },
        Granted {
            label: "an account the organization holds no record of, one permission set, GROUP",
            account_id: "123456789012".to_string(),
            permission_sets: &["Billing+Audit"],
            principal_type: "GROUP",
            principal_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
        },
        Granted {
            label: "the highest account id, three permission sets, an upper-case principal id",
            account_id: "999999999999".to_string(),
            permission_sets: &["ZZZ-Break.Glass", "Audit_1", "ReadOnly"],
            principal_type: "USER",
            principal_id: "1B2D3C4F-5A6B-7C8D-9E0F-1A2B3C4D5E6F",
        },
    ]
}

/// The four services the production router mounts, on one shared engine.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are the emulator's business, not a service's; this suite drives the
        // wire format, and the signed calls below carry a credential scope purely to name
        // the service they route to.
        router.set_request_authentication(false);
        router.register_service(PortalService::new(Arc::clone(&engine)));
        router.register_service(oidc::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(identity_center::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(organizations::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Call one IAM Identity Center (`awsJson1_1`) operation.
    async fn admin(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SWBExternalService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sso/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("the test built a well-formed request");
        parsed(self.router.dispatch(request).await)
    }

    /// Call one IAM Identity Center OIDC (`restJson1`) operation by its modelled path.
    async fn oidc(&self, path: &str, body: &Value) -> (u16, Value) {
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("https://oidc.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sso-oauth/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("the test built a well-formed request");
        parsed(self.router.dispatch(request).await)
    }

    /// Send one portal request, presenting `token` as the bearer credential the model binds
    /// the access token to.
    async fn portal(&self, method: Method, uri: &str, token: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(format!("https://{PORTAL_HOST}{uri}"))
            .header("host", PORTAL_HOST)
            .header("x-amz-sso_bearer_token", token)
            .body(Bytes::new())
            .expect("the test built a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status();
        let (_, body) = parsed(response);
        (status, body)
    }

    async fn list_accounts(&self, token: &str) -> (StatusCode, Value) {
        self.portal(Method::GET, "/assignment/accounts", token).await
    }

    async fn list_account_roles(&self, token: &str, account_id: &str) -> (StatusCode, Value) {
        self.portal(
            Method::GET,
            &format!("/assignment/roles?account_id={account_id}"),
            token,
        )
        .await
    }

    async fn role_credentials(
        &self,
        token: &str,
        account_id: &str,
        role_name: &str,
    ) -> (StatusCode, Value) {
        self.portal(
            Method::GET,
            &format!("/federation/credentials?account_id={account_id}&role_name={role_name}"),
            token,
        )
        .await
    }

    async fn logout(&self, token: &str) -> (StatusCode, Value) {
        self.portal(Method::POST, "/logout", token).await
    }

    /// The account's IAM Identity Center instance, read the way a client reads it, with
    /// permission sets enabled on it — "After permission sets are enabled, they cannot be
    /// disabled" (`UpdateInstance`), and the account's ambient instance is an account
    /// instance, on which that surface is withdrawn until they are.
    async fn instance(&self) -> (String, String) {
        let (status, body) = self.admin("ListInstances", &json!({})).await;
        assert_eq!(status, 200, "ListInstances answered {status} {body}");
        let instance = &body["Instances"][0];
        let instance_arn = text(instance, "InstanceArn");
        let identity_store_id = text(instance, "IdentityStoreId");
        let (status, body) = self
            .admin(
                "UpdateInstance",
                &json!({"InstanceArn": instance_arn, "PermissionSetsEnabled": true}),
            )
            .await;
        assert_eq!(status, 200, "UpdateInstance answered {status} {body}");
        (instance_arn, identity_store_id)
    }

    /// Create one permission set and answer the ARN the create minted.
    async fn permission_set(&self, instance_arn: &str, name: &str) -> String {
        let (status, body) = self
            .admin(
                "CreatePermissionSet",
                &json!({"InstanceArn": instance_arn, "Name": name}),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreatePermissionSet({name}) answered {status} {body}"
        );
        text(&body["PermissionSet"], "PermissionSetArn")
    }

    /// Grant one account assignment, the way an administrator does.
    async fn grant(&self, instance_arn: &str, row: &Granted, permission_set_arn: &str) {
        let (status, body) = self
            .admin(
                "CreateAccountAssignment",
                &json!({
                    "InstanceArn": instance_arn,
                    "TargetId": row.account_id,
                    "TargetType": "AWS_ACCOUNT",
                    "PermissionSetArn": permission_set_arn,
                    "PrincipalType": row.principal_type,
                    "PrincipalId": row.principal_id,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateAccountAssignment answered {status} {body}",
            row.label
        );
    }

    /// Withdraw one account assignment, the way an administrator does.
    async fn withdraw(&self, instance_arn: &str, row: &Granted, permission_set_arn: &str) {
        let (status, body) = self
            .admin(
                "DeleteAccountAssignment",
                &json!({
                    "InstanceArn": instance_arn,
                    "TargetId": row.account_id,
                    "TargetType": "AWS_ACCOUNT",
                    "PermissionSetArn": permission_set_arn,
                    "PrincipalType": row.principal_type,
                    "PrincipalId": row.principal_id,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: DeleteAccountAssignment answered {status} {body}",
            row.label
        );
    }

    /// Put the completion of every assignment grant made so far at `due_at`.
    ///
    /// Granting an assignment is asynchronous: the call is accepted `IN_PROGRESS` and the
    /// grant completes when the clock makes it due. Moving that INSTANT is the clock's
    /// business, not the wire's, so both sides of it can be observed without sleeping; the
    /// status is still derived by IAM Identity Center from the instant it holds.
    fn grants_complete_at(&self, due_at: SystemTime) {
        let state = identity_center::state::service_state(&*self.engine)
            .expect("the IAM Identity Center store is reachable");
        let mut state = state.lock();
        for operation in state.assignment_operations.values_mut() {
            operation.succeeded_due = due_at;
        }
    }

    /// An access token the emulator's OWN OIDC service issued, through the whole device
    /// flow a client (`aws sso login`) walks: register, start the authorization, have it
    /// approved at the access portal, then exchange the device code.
    async fn minted_token(&self, identity_store_id: &str) -> String {
        self.minted_sign_in(identity_store_id).await.access_token
    }

    /// The same flow, reporting everything it minted — for a case that goes on to spend the
    /// refresh token the mint issued beside the access token.
    async fn minted_sign_in(&self, identity_store_id: &str) -> MintedToken {
        let (status, body) = self
            .oidc(
                "/client/register",
                &json!({"clientName": "emulator-test", "clientType": "public"}),
            )
            .await;
        assert_eq!(status, 200, "RegisterClient answered {status} {body}");
        let client_id = text(&body, "clientId");
        let client_secret = text(&body, "clientSecret");

        let (status, body) = self
            .oidc(
                "/device_authorization",
                &json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    // The access portal of this account's Identity Center instance: the
                    // one portal a device authorization of the account can be approved at.
                    "startUrl": oidc::state::access_portal_url(identity_store_id),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "StartDeviceAuthorization answered {status} {body}"
        );
        let device_code = text(&body, "deviceCode");
        let user_code = text(&body, "userCode");

        // The user's approval at the verification page. It is not an operation of the OIDC
        // API — the page is not part of it — so it is recorded through that service's own
        // state, exactly as its own device-flow suite does.
        let approved = oidc::state::service_state(&*self.engine)
            .expect("the OIDC store is reachable")
            .lock()
            .approve_user_code(&user_code, SystemTime::now());
        assert!(approved, "the device authorization must be approvable");

        let (status, body) = self
            .oidc(
                "/token",
                &json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    "grantType": oidc::state::GRANT_TYPE_DEVICE_CODE,
                    "deviceCode": device_code,
                }),
            )
            .await;
        assert_eq!(status, 200, "CreateToken answered {status} {body}");
        MintedToken {
            access_token: text(&body, "accessToken"),
            refresh_token: text(&body, "refreshToken"),
            client_id,
            client_secret,
        }
    }

    /// Spend `minted`'s refresh token, as a client whose access token is about to run out
    /// does, and answer the access token the rotation issued. The rotation supersedes the
    /// previous access token: the mint drops it.
    async fn refreshed(&self, minted: &MintedToken) -> String {
        let (status, body) = self
            .oidc(
                "/token",
                &json!({
                    "clientId": minted.client_id,
                    "clientSecret": minted.client_secret,
                    "grantType": oidc::state::GRANT_TYPE_REFRESH_TOKEN,
                    "refreshToken": minted.refresh_token,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateToken on the refresh-token grant answered {status} {body}"
        );
        text(&body, "accessToken")
    }

    /// Age the emulator's record of `access_token` past its own advertised lifetime, which
    /// is how a case reaches an expired token without waiting hours for one.
    fn expire_token(&self, access_token: &str) {
        let state = oidc::state::service_state(&*self.engine).expect("the OIDC store");
        let mut state = state.lock();
        let issued = state
            .access_tokens
            .get_mut(access_token)
            .expect("the token the mint issued is on record");
        let lifetime = u64::try_from(issued.expires_in).expect("a modelled lifetime is positive");
        issued.issued_at -= Duration::from_secs(lifetime + 1);
    }

    /// The name and email address the ORGANIZATION holds for `account_id`, which is what
    /// the portal must report for it, and `None` for an account it holds no record of.
    fn organization_account(&self, account_id: &str) -> Option<(String, String)> {
        let state = organizations::state::service_state(&*self.engine)
            .expect("the Organizations store is reachable");
        let state = state.lock();
        state
            .accounts
            .get(account_id)
            .map(|member| (member.name.clone(), member.email.clone()))
    }

    /// The Identity Center subject the role credential `access_key_id` speaks for, as the
    /// portal's store resolves it — what a service behind the portal authorizes a request
    /// by.
    fn signed_in_subject(&self, access_key_id: &str) -> Option<String> {
        self.portal_store()
            .lock()
            .signed_in_subject(access_key_id, SystemTime::now())
            .map(str::to_string)
    }

    /// This service's own store, for the cases whose subject is a record only it keeps.
    fn portal_store(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_sso_swbportalservice::state::ComAmazonawsSsoSwbportalserviceState,
    > {
        service_com_amazonaws_sso_swbportalservice::state::service_state(&*self.engine)
            .expect("the portal store is reachable")
    }

    /// Whether the portal keeps a record of the sign-in `access_token` names — which it
    /// does only once a `Logout` or a credential mint made it record one.
    fn records_sign_in(&self, access_token: &str) -> bool {
        self.portal_store().lock().sessions.contains_key(access_token)
    }

    /// Put every sign-in record this store keeps past its own expiry, so the reclaim the
    /// clock owes them can be observed without the suite sleeping out a token's lifetime.
    fn sign_in_records_expired(&self) {
        let spent = SystemTime::now() - Duration::from_secs(1);
        for session in self.portal_store().lock().sessions.values_mut() {
            session.expires_at = spent;
        }
    }

    /// The same, for the role credentials this portal minted: their advertised expiration
    /// moved into the past.
    fn credentials_expired(&self) {
        let spent = service_com_amazonaws_sso_swbportalservice::state::epoch_millis(
            SystemTime::now() - Duration::from_secs(1),
        );
        for record in self.portal_store().lock().issued_credentials.values_mut() {
            record.expiration_millis = spent;
        }
    }
}

/// Everything one walk of the device flow minted: the access token a portal request
/// presents, the refresh token beside it, and the registration both belong to.
struct MintedToken {
    access_token: String,
    refresh_token: String,
    client_id: String,
    client_secret: String,
}

/// The status and parsed body of a response; a body that is not JSON fails the case rather
/// than being swallowed.
fn parsed(response: Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "the response body must be JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, body)
}

/// One required string member of a response.
fn text(body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} is missing from {body}"))
        .to_string()
}

/// The wire error code (`__type`) of a failure response, without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `accountId` of every account a `ListAccounts` response reports, in order.
fn reported_accounts(body: &Value) -> Vec<String> {
    body["accountList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|account| text(account, "accountId"))
        .collect()
}

/// The `roleName` of every role a `ListAccountRoles` response reports, in order.
fn reported_roles(body: &Value) -> Vec<String> {
    body["roleList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|role| text(role, "roleName"))
        .collect()
}

/// An emulator with every row of [`granted`] assigned and every grant complete.
struct Assigned {
    emulator: Emulator,
    instance_arn: String,
    identity_store_id: String,
    /// The permission-set ARNs of each row, in the row's own order.
    permission_sets: Vec<Vec<String>>,
    /// An access token the emulator's own mint issued.
    token: String,
}

/// Set an emulator up with every row of `rows` assigned and its grant complete.
async fn assigned_emulator(rows: &[Granted]) -> Assigned {
    let emulator = Emulator::new();
    let (instance_arn, identity_store_id) = emulator.instance().await;
    let mut permission_sets: Vec<Vec<String>> = Vec::new();
    for row in rows {
        let mut row_arns = Vec::new();
        for name in row.permission_sets {
            // One permission set per (account, role) pair, so each account's roles are
            // its own and a listing cannot pass by reporting one shared set.
            let arn = emulator
                .permission_set(&instance_arn, &role_name(name, &row.account_id))
                .await;
            emulator.grant(&instance_arn, row, &arn).await;
            row_arns.push(arn);
        }
        permission_sets.push(row_arns);
    }
    // Every grant made above has completed.
    emulator.grants_complete_at(SystemTime::now() - Duration::from_secs(1));
    let token = emulator.minted_token(&identity_store_id).await;
    Assigned {
        emulator,
        instance_arn,
        identity_store_id,
        permission_sets,
        token,
    }
}

/// The role name the portal must report for the permission set `name` of `account_id`: the
/// set's own name, which [`assigned_emulator`] makes unique per account.
fn role_name(name: &str, account_id: &str) -> String {
    format!("{name}-{account_id}")
}

/// The role names of `row`, ascending — what a role listing of its account must report.
fn expected_roles(row: &Granted) -> Vec<String> {
    let mut roles: Vec<String> = row
        .permission_sets
        .iter()
        .map(|name| role_name(name, &row.account_id))
        .collect();
    roles.sort();
    roles
}

/// RULE: a token the emulator's own `CreateToken` issued names a sign-in, and that sign-in
/// reports exactly the IAM Identity Center assignments that stand — their accounts, the
/// organization's name and email for each, and the permission-set name of each assignment
/// as the role.
#[tokio::test]
async fn a_minted_token_reports_the_assignments_identity_center_holds() {
    let rows = granted();
    let assigned = assigned_emulator(&rows).await;
    let emulator = &assigned.emulator;
    let token = &assigned.token;

    let (status, body) = emulator.list_accounts(token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "ListAccounts with a token the emulator minted must succeed, got {status} {body}"
    );
    let mut expected: Vec<String> = rows.iter().map(|row| row.account_id.clone()).collect();
    expected.sort();
    assert_eq!(
        reported_accounts(&body),
        expected,
        "ListAccounts must report every assigned account, ascending, got {body}"
    );

    for account in body["accountList"].as_array().cloned().unwrap_or_default() {
        let account_id = text(&account, "accountId");
        let held = emulator.organization_account(&account_id);
        assert_eq!(
            (
                account["accountName"].as_str().map(str::to_string),
                account["emailAddress"].as_str().map(str::to_string)
            ),
            (
                held.as_ref().map(|(name, _)| name.clone()),
                held.as_ref().map(|(_, email)| email.clone())
            ),
            "case {account_id:?}: the account's name and email must be the organization's \
             records of it ({held:?}), got {account}"
        );
    }

    // The management account IS one the organization holds a record of, so the assertion
    // above is not a pair of absences: the name and email reported for it are the
    // organization's own, and an account the organization holds no record of carries
    // neither member.
    let management_id = engine::emulator_account_id();
    let reported = body["accountList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|account| text(account, "accountId") == management_id)
        .unwrap_or_else(|| panic!("the management account must be reported: {body}"));
    let (name, email) = emulator
        .organization_account(management_id)
        .expect("the organization holds a record of its management account");
    assert!(
        !name.is_empty() && !email.is_empty(),
        "the organization's record of the management account must carry a name and an email"
    );
    assert_eq!(
        (
            reported["accountName"].as_str(),
            reported["emailAddress"].as_str()
        ),
        (Some(name.as_str()), Some(email.as_str())),
        "the management account must be reported with the organization's name and email, \
         got {reported}"
    );
    let unknown_to_the_organization = rows
        .iter()
        .find(|row| emulator.organization_account(&row.account_id).is_none())
        .expect("the fixture holds an account the organization has no record of");
    let reported = body["accountList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|account| text(account, "accountId") == unknown_to_the_organization.account_id)
        .unwrap_or_else(|| panic!("every assigned account must be reported: {body}"));
    assert_eq!(
        (
            reported.get("accountName"),
            reported.get("emailAddress")
        ),
        (None, None),
        "an account the emulator holds no record of carries no name and no email, got \
         {reported}"
    );

    for row in &rows {
        let (status, body) = emulator.list_account_roles(token, &row.account_id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: ListAccountRoles must succeed, got {status} {body}",
            row.label
        );
        assert_eq!(
            reported_roles(&body),
            expected_roles(row),
            "case {:?}: expected the permission-set names assigned there, ascending, got {body}",
            row.label
        );
    }

    // An account nobody assigned to this sign-in does not exist as far as it is concerned,
    // and neither does a role nobody assigned in an account it CAN see.
    let (status, body) = emulator.list_account_roles(token, "555555555555").await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (StatusCode::NOT_FOUND, "ResourceNotFoundException"),
        "an unassigned account must be reported absent, got {status} {body}"
    );
    let (status, body) = emulator
        .role_credentials(token, &rows[0].account_id, "NotAssignedHere")
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (StatusCode::NOT_FOUND, "ResourceNotFoundException"),
        "an unassigned role must be reported absent, got {status} {body}"
    );

    // And the credential the portal mints for an assignment it does report speaks for the
    // sign-in it was issued to, which is what a service behind the portal authorizes by.
    let assigned_role = role_name(rows[0].permission_sets[0], &rows[0].account_id);
    let (status, body) = emulator
        .role_credentials(token, &rows[0].account_id, &assigned_role)
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GetRoleCredentials for an assigned role must succeed, got {status} {body}"
    );
    let access_key_id = text(&body["roleCredentials"], "accessKeyId");
    assert!(
        emulator.signed_in_subject(&access_key_id).is_some(),
        "the minted credential must speak for the sign-in it was issued to, got {:?}",
        emulator.signed_in_subject(&access_key_id)
    );
}

/// RULE: an assignment is in force once the asynchronous grant that made it has completed,
/// and not before — the portal does not offer an account whose grant is still running.
#[tokio::test]
async fn an_assignment_is_reported_once_its_grant_has_completed() {
    let rows = granted();
    let emulator = Emulator::new();
    let (instance_arn, identity_store_id) = emulator.instance().await;
    let token = emulator.minted_token(&identity_store_id).await;

    // Nothing is assigned yet: the sign-in EXISTS — the token is one the emulator minted —
    // and reports no account, which is not the same answer as refusing the token.
    let (status, body) = emulator.list_accounts(&token).await;
    assert_eq!(
        (status, reported_accounts(&body)),
        (StatusCode::OK, Vec::new()),
        "a sign-in with no assignments must report an empty list, got {status} {body}"
    );

    for row in &rows {
        let arn = emulator
            .permission_set(&instance_arn, &role_name(row.permission_sets[0], &row.account_id))
            .await;
        emulator.grant(&instance_arn, row, &arn).await;
    }

    // Every grant is still running.
    emulator.grants_complete_at(SystemTime::now() + Duration::from_secs(3600));
    let (status, body) = emulator.list_accounts(&token).await;
    assert_eq!(
        (status, reported_accounts(&body)),
        (StatusCode::OK, Vec::new()),
        "an assignment whose grant is still IN_PROGRESS must not be reported, got {status} {body}"
    );
    for row in &rows {
        let (status, body) = emulator.list_account_roles(&token, &row.account_id).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (StatusCode::NOT_FOUND, "ResourceNotFoundException"),
            "case {:?}: an account whose grant is still running is absent, got {status} {body}",
            row.label
        );
    }

    // The clock makes every grant due.
    emulator.grants_complete_at(SystemTime::now() - Duration::from_secs(1));
    let (status, body) = emulator.list_accounts(&token).await;
    let mut expected: Vec<String> = rows.iter().map(|row| row.account_id.clone()).collect();
    expected.sort();
    assert_eq!(
        (status, reported_accounts(&body)),
        (StatusCode::OK, expected),
        "a completed grant must be reported, got {status} {body}"
    );
}

/// RULE: the portal reports the assignments that stand at the instant it is asked —
/// withdrawing one stops it being reported, with nothing propagating the withdrawal.
#[tokio::test]
async fn a_withdrawn_assignment_stops_being_reported() {
    let rows = granted();
    let assigned = assigned_emulator(&rows).await;
    let emulator = &assigned.emulator;
    let token = &assigned.token;

    // The row with the most permission sets: withdrawing one of them removes that role and
    // leaves the account, and withdrawing the last removes the account itself.
    let (index, row) = rows
        .iter()
        .enumerate()
        .max_by_key(|(_, row)| row.permission_sets.len())
        .expect("the fixture has rows");
    assert!(
        row.permission_sets.len() > 1,
        "the fixture must hold an account with several permission sets"
    );
    let row_arns = &assigned.permission_sets[index];

    for (withdrawn, arn) in row_arns.iter().enumerate() {
        emulator.withdraw(&assigned.instance_arn, row, arn).await;
        // The roles still assigned there: the permission sets this loop has not withdrawn.
        let mut left: Vec<String> = row.permission_sets[withdrawn + 1..]
            .iter()
            .map(|name| role_name(name, &row.account_id))
            .collect();
        left.sort();
        let (status, body) = emulator.list_account_roles(token, &row.account_id).await;
        if left.is_empty() {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (StatusCode::NOT_FOUND, "ResourceNotFoundException"),
                "case {:?}: an account with no assignment left is absent, got {status} {body}",
                row.label
            );
        } else {
            assert_eq!(
                (status, reported_roles(&body)),
                (StatusCode::OK, left.clone()),
                "case {:?}: expected the roles still assigned ({left:?}), got {status} {body}",
                row.label
            );
        }
    }

    let (status, body) = emulator.list_accounts(token).await;
    let mut expected: Vec<String> = rows
        .iter()
        .map(|other| other.account_id.clone())
        .filter(|account_id| *account_id != row.account_id)
        .collect();
    expected.sort();
    assert_eq!(
        (status, reported_accounts(&body)),
        (StatusCode::OK, expected),
        "an account whose every assignment was withdrawn must not be reported, got {status} {body}"
    );
}

/// RULE: only a token the emulator itself issued, and only one still inside its own
/// lifetime, names a sign-in; everything else is the portal's one authentication refusal.
#[tokio::test]
async fn only_a_token_the_emulator_issued_and_still_holds_names_a_sign_in() {
    let rows = granted();
    let assigned = assigned_emulator(&rows).await;
    let emulator = &assigned.emulator;
    let token = assigned.token.clone();
    // A second registration's token: a different client, the same account's assignments.
    let second_token = emulator.minted_token(&assigned.identity_store_id).await;
    let expired = emulator.minted_token(&assigned.identity_store_id).await;
    emulator.expire_token(&expired);

    // (case, the token presented, served)
    let presented: Vec<(&str, String, bool)> = vec![
        ("a token the emulator's own CreateToken issued", token.clone(), true),
        (
            "a token issued to a second registration of the same emulator",
            second_token,
            true,
        ),
        (
            "a token the emulator issued whose lifetime has run out",
            expired,
            false,
        ),
        (
            "a bearer value the emulator never issued",
            "not-a-token-this-emulator-ever-issued".to_string(),
            false,
        ),
        (
            "another minted token's value with one character changed",
            flip_last(&token),
            false,
        ),
    ];

    let mut assigned: Vec<String> = rows.iter().map(|row| row.account_id.clone()).collect();
    assigned.sort();
    for (case, presented, served) in presented {
        let (status, body) = emulator.list_accounts(&presented).await;
        if served {
            assert_eq!(
                (status, reported_accounts(&body)),
                (StatusCode::OK, assigned.clone()),
                "case {case:?}: expected the assigned accounts, got {status} {body}"
            );
        } else {
            assert_eq!(
                (
                    status,
                    error_code(&body),
                    body["message"].as_str().unwrap_or_default().to_string()
                ),
                (
                    StatusCode::UNAUTHORIZED,
                    "UnauthorizedException".to_string(),
                    INVALID_SESSION_MESSAGE.to_string()
                ),
                "case {case:?}: expected the portal's authentication refusal, got {status} {body}"
            );
        }
    }
}

/// RULE: `Logout` invalidates the sign-in a token names, so every later portal request
/// presenting it is unauthorized — even though the token itself is still one the emulator
/// minted — while the role credentials already issued to that sign-in go on working for the
/// rest of their own duration.
#[tokio::test]
async fn logout_ends_a_minted_sign_in_for_good() {
    let rows = granted();
    let assigned = assigned_emulator(&rows).await;
    let emulator = &assigned.emulator;
    let token = &assigned.token;

    let assigned_role = role_name(rows[0].permission_sets[0], &rows[0].account_id);
    let (status, body) = emulator
        .role_credentials(token, &rows[0].account_id, &assigned_role)
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GetRoleCredentials must succeed before the logout, got {status} {body}"
    );
    let access_key_id = text(&body["roleCredentials"], "accessKeyId");

    let (status, body) = emulator.logout(token).await;
    assert_eq!(status, StatusCode::OK, "Logout answered {status} {body}");

    // Every operation, twice: the sign-in stays ended, and a second logout is refused too.
    for attempt in 1..=2 {
        for (operation, status, body) in [
            ("ListAccounts", emulator.list_accounts(token).await),
            (
                "ListAccountRoles",
                emulator.list_account_roles(token, &rows[0].account_id).await,
            ),
            (
                "GetRoleCredentials",
                emulator
                    .role_credentials(token, &rows[0].account_id, &assigned_role)
                    .await,
            ),
            ("Logout", emulator.logout(token).await),
        ]
        .map(|(operation, (status, body))| (operation, status, body))
        {
            assert_eq!(
                (
                    status,
                    error_code(&body),
                    body["message"].as_str().unwrap_or_default().to_string()
                ),
                (
                    StatusCode::UNAUTHORIZED,
                    "UnauthorizedException".to_string(),
                    INVALID_SESSION_MESSAGE.to_string()
                ),
                "attempt {attempt}: {operation} after a logout must be unauthorized, got \
                 {status} {body}"
            );
        }
    }

    assert!(
        emulator.signed_in_subject(&access_key_id).is_some(),
        "a credential minted before the logout still speaks for that sign-in for the rest \
         of its own duration, got {:?}",
        emulator.signed_in_subject(&access_key_id)
    );
}

/// RULE: the mint's own record is what says whether a token still names a sign-in, so a
/// token the mint no longer holds — one a refresh superseded — is refused from that instant,
/// even though this store had been made to record that sign-in, while the token the
/// rotation issued is served and the credential minted before it goes on working for the
/// rest of its own duration.
#[tokio::test]
async fn a_superseded_token_stops_being_served() {
    let rows = granted();
    let assigned = assigned_emulator(&rows).await;
    let emulator = &assigned.emulator;
    let minted = emulator.minted_sign_in(&assigned.identity_store_id).await;
    let assigned_role = role_name(rows[0].permission_sets[0], &rows[0].account_id);

    // A credential mint, so this store holds a record of the sign-in before the refresh.
    let (status, body) = emulator
        .role_credentials(&minted.access_token, &rows[0].account_id, &assigned_role)
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GetRoleCredentials answered {status} {body}"
    );
    let access_key_id = text(&body["roleCredentials"], "accessKeyId");
    assert!(
        emulator.records_sign_in(&minted.access_token),
        "the mint recorded the sign-in it issued a credential to"
    );

    let rotated = emulator.refreshed(&minted).await;
    let mut expected: Vec<String> = rows.iter().map(|row| row.account_id.clone()).collect();
    expected.sort();
    let (status, body) = emulator.list_accounts(&rotated).await;
    assert_eq!(
        (status, reported_accounts(&body)),
        (StatusCode::OK, expected),
        "the token the rotation issued must be served, got {status} {body}"
    );

    for (operation, status, body) in [
        (
            "ListAccounts",
            emulator.list_accounts(&minted.access_token).await,
        ),
        (
            "ListAccountRoles",
            emulator
                .list_account_roles(&minted.access_token, &rows[0].account_id)
                .await,
        ),
        (
            "GetRoleCredentials",
            emulator
                .role_credentials(&minted.access_token, &rows[0].account_id, &assigned_role)
                .await,
        ),
        ("Logout", emulator.logout(&minted.access_token).await),
    ]
    .map(|(operation, (status, body))| (operation, status, body))
    {
        assert_eq!(
            (
                status,
                error_code(&body),
                body["message"].as_str().unwrap_or_default().to_string()
            ),
            (
                StatusCode::UNAUTHORIZED,
                "UnauthorizedException".to_string(),
                INVALID_SESSION_MESSAGE.to_string()
            ),
            "{operation} on a superseded token must be unauthorized, got {status} {body}"
        );
    }

    assert!(
        emulator.signed_in_subject(&access_key_id).is_some(),
        "the credential minted before the refresh still speaks for that sign-in"
    );
}

/// `value` with its last character replaced, so the result is a bearer value of the same
/// shape that the emulator never issued.
fn flip_last(value: &str) -> String {
    let mut flipped: Vec<char> = value.chars().collect();
    if let Some(last) = flipped.last_mut() {
        *last = if *last == 'a' { 'b' } else { 'a' };
    }
    flipped.into_iter().collect()
}

/// RULE: a record this store keeps of a sign-in is reclaimed once the clock has made it
/// spent — its own expiry passed and no role credential it was issued to still usable — so
/// presenting tokens cannot grow the store for the life of the process, while a record a
/// usable credential still speaks through is kept.
#[tokio::test]
async fn a_spent_sign_in_record_is_reclaimed() {
    let rows = granted();
    let assigned = assigned_emulator(&rows).await;
    let emulator = &assigned.emulator;
    let assigned_role = role_name(rows[0].permission_sets[0], &rows[0].account_id);

    // The two requests that make this store record a sign-in: a mint, and a logout.
    let with_credential = assigned.token.clone();
    let (status, body) = emulator
        .role_credentials(&with_credential, &rows[0].account_id, &assigned_role)
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GetRoleCredentials answered {status} {body}"
    );
    let access_key_id = text(&body["roleCredentials"], "accessKeyId");

    let logged_out = emulator.minted_token(&assigned.identity_store_id).await;
    let (status, body) = emulator.logout(&logged_out).await;
    assert_eq!(status, StatusCode::OK, "Logout answered {status} {body}");
    for token in [&with_credential, &logged_out] {
        assert!(
            emulator.records_sign_in(token),
            "the portal must hold a record of a sign-in it minted for or logged out"
        );
    }

    // The clock makes both records spent. A third, live sign-in makes the request that
    // reaches the sweep, so nothing about the two under test triggers their own reclaim.
    let live = emulator.minted_token(&assigned.identity_store_id).await;
    emulator.sign_in_records_expired();
    let (status, body) = emulator.list_accounts(&live).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the live sign-in must still be served, got {status} {body}"
    );
    assert!(
        !emulator.records_sign_in(&logged_out),
        "a spent sign-in record with no credential of its own must be reclaimed"
    );
    assert!(
        emulator.records_sign_in(&with_credential),
        "a spent sign-in record a usable credential still speaks through must be kept"
    );
    assert!(
        emulator.signed_in_subject(&access_key_id).is_some(),
        "that credential must still resolve the identity it was issued to"
    );

    // And once the credential is spent too, nothing holds the record back.
    emulator.credentials_expired();
    let (status, body) = emulator.list_accounts(&live).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the live sign-in must still be served, got {status} {body}"
    );
    assert!(
        !emulator.records_sign_in(&with_credential),
        "a spent sign-in record whose credentials are spent too must be reclaimed"
    );
    assert!(
        emulator.signed_in_subject(&access_key_id).is_none(),
        "a spent credential resolves no identity"
    );
}
