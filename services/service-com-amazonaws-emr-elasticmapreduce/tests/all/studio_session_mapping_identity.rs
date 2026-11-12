// SPDX-License-Identifier: CC0-1.0

//! The IDENTITY an EMR Studio session mapping maps, end to end over the wire, with the
//! IAM Identity Center identity store mounted beside Amazon EMR on ONE engine.
//!
//! THE RULE these cases assert: a session mapping names ONE identity of the identity
//! store, addressed either by "the globally unique identifier (GUID) of the user or
//! group" (`SessionMappingDetail$IdentityId` doc) or by "the name of the user or group"
//! (`SessionMappingDetail$IdentityName` doc) — so whichever member the create was given,
//! the mapping carries both once the identity resolved in the store that owns it, a read
//! reports both, and either member addresses the mapping.
//!
//! Every identity here is created through the identity store's OWN API
//! (`CreateUser`/`CreateGroup`), and every mapping is read back through
//! `GetStudioSessionMapping` and `ListStudioSessionMappings`, so nothing below is derived
//! from a value this test invented: a name that is reported is a name the other service
//! holds.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The identity store the identities live in, in the form the model's `IdentityStoreId`
/// pattern gives one (`d-` plus ten lowercase hex characters).
const IDENTITY_STORE: &str = "d-1234567890";

/// The session policy every mapping below carries.
const SESSION_POLICY: &str = "arn:aws:iam::aws:policy/AmazonEMRFullAccessPolicy_v2";

static NULL: Value = Value::Null;

/// Amazon EMR and the IAM Identity Center identity store on one engine.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_identitystore_awsidentitystore::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    Emulator { router }
}

impl Emulator {
    /// One `awsJson1_1` call to the service published under `host`, with the target
    /// prefix `prefix`.
    async fn call(
        &self,
        host: &str,
        signing_name: &str,
        prefix: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261109/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    async fn emr(&self, case: &str, operation: &str, body: Value) -> Value {
        let (status, payload) = self
            .call(
                &format!("elasticmapreduce.{REGION}.amazonaws.com"),
                "elasticmapreduce",
                "ElasticMapReduce",
                operation,
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {payload}"
        );
        payload
    }

    async fn emr_call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call(
            &format!("elasticmapreduce.{REGION}.amazonaws.com"),
            "elasticmapreduce",
            "ElasticMapReduce",
            operation,
            body,
        )
        .await
    }

    async fn identity_store(&self, case: &str, operation: &str, body: Value) -> Value {
        let (status, payload) = self
            .call(
                &format!("identitystore.{REGION}.amazonaws.com"),
                "identitystore",
                "AWSIdentityStore",
                operation,
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {payload}"
        );
        payload
    }

    /// A Studio to map identities into, answering its id.
    async fn studio(&self, name: &str) -> String {
        let out = self
            .emr(
                name,
                "CreateStudio",
                json!({
                    "Name": name,
                    "AuthMode": "SSO",
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d"],
                    "ServiceRole": "EMR_Studio_ServiceRole",
                    "UserRole": "EMR_Studio_UserRole",
                    "WorkspaceSecurityGroupId": "sg-0a1b2c3d",
                    "EngineSecurityGroupId": "sg-1a2b3c4d",
                    "DefaultS3Location": "s3://emr-studio/backup/",
                }),
            )
            .await;
        out["StudioId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: expected a StudioId, got {out}"))
            .to_string()
    }
}

fn at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key).unwrap_or(&NULL);
    }
    cursor
}

/// Which member of the identity a create names it by: the rule holds either way round.
#[derive(Debug, Clone, Copy)]
enum By {
    Id,
    Name,
}

/// One row: the kind of identity, the name it is created under in the identity store, and
/// which member the session mapping is created by.
#[derive(Debug)]
struct Row {
    case: &'static str,
    identity_type: &'static str,
    identity_name: &'static str,
    by: By,
}

/// THE RULE: a mapping created by either member of its identity reports BOTH members, and
/// is addressed by either one.
///
/// The rows differ along what the rule generalises over: the identity KIND (a group and a
/// user, whose names are different members of different shapes in the identity store) and
/// which member the create named it by.
#[tokio::test]
async fn a_session_mapping_reports_both_members_of_the_identity_it_maps() {
    let emulator = emulator();
    let studio_id = emulator.studio("mapped").await;

    let rows = [
        Row {
            case: "a group mapped by its GUID",
            identity_type: "GROUP",
            identity_name: "emr-studio-analysts",
            by: By::Id,
        },
        Row {
            case: "a group mapped by its name",
            identity_type: "GROUP",
            identity_name: "emr-studio-engineers",
            by: By::Name,
        },
        Row {
            case: "a user mapped by its GUID",
            identity_type: "USER",
            identity_name: "analyst@example.com",
            by: By::Id,
        },
        Row {
            case: "a user mapped by its name",
            identity_type: "USER",
            identity_name: "engineer@example.com",
            by: By::Name,
        },
    ];

    for row in &rows {
        let Row {
            case,
            identity_type,
            identity_name,
            by,
        } = row;

        // The identity, created in the store that OWNS it.
        let identity_id = match *identity_type {
            "GROUP" => {
                let created = emulator
                    .identity_store(
                        case,
                        "CreateGroup",
                        json!({
                            "IdentityStoreId": IDENTITY_STORE,
                            "DisplayName": identity_name,
                        }),
                    )
                    .await;
                created["GroupId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a GroupId, got {created}"))
                    .to_string()
            }
            _ => {
                let created = emulator
                    .identity_store(
                        case,
                        "CreateUser",
                        json!({
                            "IdentityStoreId": IDENTITY_STORE,
                            "UserName": identity_name,
                        }),
                    )
                    .await;
                created["UserId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a UserId, got {created}"))
                    .to_string()
            }
        };

        // The mapping, created by ONE member of that identity.
        let mut create = json!({
            "StudioId": &studio_id,
            "IdentityType": identity_type,
            "SessionPolicyArn": SESSION_POLICY,
        });
        match by {
            By::Id => create["IdentityId"] = json!(&identity_id),
            By::Name => create["IdentityName"] = json!(identity_name),
        }
        emulator
            .emr(case, "CreateStudioSessionMapping", create)
            .await;

        // Read back by EACH member: one mapping, addressed either way, reporting both.
        for (addressed_by, member, value) in [
            ("its GUID", "IdentityId", identity_id.clone()),
            ("its name", "IdentityName", (*identity_name).to_string()),
        ] {
            let case = format!("{case}, read back by {addressed_by}");
            let mut request = json!({
                "StudioId": &studio_id,
                "IdentityType": identity_type,
            });
            request[member] = json!(value);
            let mapping = emulator
                .emr(&case, "GetStudioSessionMapping", request)
                .await["SessionMapping"]
                .clone();
            assert_eq!(
                at(&mapping, &["IdentityName"]).as_str(),
                Some(*identity_name),
                "case {case:?}: the mapping must report the identity's name, got {mapping}"
            );
            assert_eq!(
                at(&mapping, &["IdentityId"]).as_str(),
                Some(identity_id.as_str()),
                "case {case:?}: the mapping must report the identity's GUID, got {mapping}"
            );
            assert_eq!(
                at(&mapping, &["IdentityType"]).as_str(),
                Some(*identity_type),
                "case {case:?}: the mapping must report the identity's kind, got {mapping}"
            );
            assert_eq!(
                at(&mapping, &["SessionPolicyArn"]).as_str(),
                Some(SESSION_POLICY),
                "case {case:?}: the mapping must report the policy it was created with, \
                 got {mapping}"
            );
        }
    }

    // One mapping per identity, each listed with both members: the create was not
    // duplicated by being addressed two ways.
    let listed = emulator
        .emr("the whole table", "ListStudioSessionMappings", json!({ "StudioId": &studio_id }))
        .await;
    let mappings = at(&listed, &["SessionMappings"])
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        mappings.len(),
        rows.len(),
        "case {:?}: expected one mapping per identity, got {listed}",
        "the whole table"
    );
    for row in &rows {
        let found = mappings
            .iter()
            .find(|mapping| at(mapping, &["IdentityName"]).as_str() == Some(row.identity_name))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the listing must report the identity's name, got {listed}",
                    row.case
                )
            });
        assert!(
            at(found, &["IdentityId"]).as_str().is_some(),
            "case {:?}: the listing must report the identity's GUID too, got {found}",
            row.case
        );
    }
}

/// THE RULE's other side: nothing is invented for an identity the store has no record of
/// — the mapping keeps exactly the member the request named, and the other stays absent —
/// and an identity the store holds is still mapped once, however it is addressed.
#[tokio::test]
async fn an_identity_the_store_does_not_hold_is_reported_as_named_and_nothing_else() {
    let emulator = emulator();
    let studio_id = emulator.studio("unresolved").await;

    // Rows: a name no identity carries, and a GUID no identity carries — the two members
    // a create may name an identity by.
    for (case, member, value, other) in [
        (
            "a group name the identity store has no record of",
            "IdentityName",
            "never-provisioned-group",
            "IdentityId",
        ),
        (
            "a GUID the identity store has no record of",
            "IdentityId",
            "11111111-2222-3333-4444-555555555555",
            "IdentityName",
        ),
    ] {
        let mut create = json!({
            "StudioId": &studio_id,
            "IdentityType": "GROUP",
            "SessionPolicyArn": SESSION_POLICY,
        });
        create[member] = json!(value);
        emulator
            .emr(case, "CreateStudioSessionMapping", create)
            .await;

        let mut request = json!({ "StudioId": &studio_id, "IdentityType": "GROUP" });
        request[member] = json!(value);
        let mapping = emulator.emr(case, "GetStudioSessionMapping", request).await
            ["SessionMapping"]
            .clone();
        assert_eq!(
            at(&mapping, &[member]).as_str(),
            Some(value),
            "case {case:?}: the mapping must report the member the create named, got {mapping}"
        );
        assert!(
            at(&mapping, &[other]).as_str().is_none(),
            "case {case:?}: nothing may be invented for the member the store could not \
             resolve, got {mapping}"
        );
    }

    // A second create for the SAME identity is refused whichever member addresses it: one
    // identity, one mapping.
    let group = emulator
        .identity_store(
            "a resolvable group",
            "CreateGroup",
            json!({ "IdentityStoreId": IDENTITY_STORE, "DisplayName": "emr-studio-once" }),
        )
        .await;
    let group_id = group["GroupId"].as_str().expect("a GroupId").to_string();
    emulator
        .emr(
            "mapped once by name",
            "CreateStudioSessionMapping",
            json!({
                "StudioId": &studio_id,
                "IdentityType": "GROUP",
                "IdentityName": "emr-studio-once",
                "SessionPolicyArn": SESSION_POLICY,
            }),
        )
        .await;
    let (status, payload) = emulator
        .emr_call(
            "CreateStudioSessionMapping",
            json!({
                "StudioId": &studio_id,
                "IdentityType": "GROUP",
                "IdentityId": &group_id,
                "SessionPolicyArn": SESSION_POLICY,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {:?}: expected HTTP 400, got {status} with {payload}",
        "the same identity mapped again by its GUID"
    );

    // And deleting it by the OTHER member removes that one mapping.
    emulator
        .emr(
            "deleted by its GUID",
            "DeleteStudioSessionMapping",
            json!({
                "StudioId": &studio_id,
                "IdentityType": "GROUP",
                "IdentityId": &group_id,
            }),
        )
        .await;
    let (status, payload) = emulator
        .emr_call(
            "GetStudioSessionMapping",
            json!({
                "StudioId": &studio_id,
                "IdentityType": "GROUP",
                "IdentityName": "emr-studio-once",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {:?}: a deleted mapping is gone whichever member asks for it, got {status} \
         with {payload}",
        "read back by its name after the delete"
    );
}
