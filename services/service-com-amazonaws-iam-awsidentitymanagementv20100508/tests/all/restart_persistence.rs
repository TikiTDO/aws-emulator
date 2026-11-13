// SPDX-License-Identifier: CC0-1.0

//! IAM state survives a restart through `service_common::persist`: a role with its trust and
//! inline policies, and a tagged user, read back unchanged from a fresh engine restored from
//! the snapshot.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::ComAmazonawsIamAwsidentitymanagementv20100508State as State;
use service_common::persist::Persisted;

use crate::user_lifecycle::{call, element, router_with_engine};

const TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ecs-tasks.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;
const INLINE: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

#[tokio::test]
async fn a_role_and_a_user_from_before_a_restart_read_back_after_it() {
    let (before, engine) = router_with_engine();
    let (status, created) = call(
        &before,
        "CreateRole",
        &[
            ("RoleName", "TaskRole"),
            ("AssumeRolePolicyDocument", TRUST),
        ],
    )
    .await;
    assert_eq!(status, 200, "{created}");
    let role_id = element(&created, "RoleId");
    let created_at = element(&created, "CreateDate");
    let (status, body) = call(
        &before,
        "PutRolePolicy",
        &[
            ("RoleName", "TaskRole"),
            ("PolicyName", "read"),
            ("PolicyDocument", INLINE),
        ],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = call(
        &before,
        "CreateUser",
        &[
            ("UserName", "deployer"),
            ("Tags.member.1.Key", "team"),
            ("Tags.member.1.Value", "night"),
        ],
    )
    .await;
    assert_eq!(status, 200, "{body}");

    // What a shutdown writes, through the same JSON text the file holds.
    let snapshot = engine.state::<State>().lock().snapshot();
    let text = serde_json::to_string(&snapshot).unwrap();
    let restored = State::restore(State::VERSION, serde_json::from_str(&text).unwrap()).unwrap();

    let (after, fresh) = router_with_engine();
    *fresh.state::<State>().lock() = restored;
    let (status, role) = call(&after, "GetRole", &[("RoleName", "TaskRole")]).await;
    assert_eq!(status, 200, "{role}");
    assert_eq!(element(&role, "RoleId"), role_id);
    assert_eq!(element(&role, "CreateDate"), created_at);
    let (status, policy) = call(
        &after,
        "GetRolePolicy",
        &[("RoleName", "TaskRole"), ("PolicyName", "read")],
    )
    .await;
    assert_eq!(status, 200, "{policy}");
    assert!(
        element(&policy, "PolicyDocument").contains("s3%3AGetObject")
            || element(&policy, "PolicyDocument").contains("s3:GetObject")
    );
    let (status, tags) = call(&after, "ListUserTags", &[("UserName", "deployer")]).await;
    assert_eq!(status, 200, "{tags}");
    assert_eq!(element(&tags, "Value"), "night");
}
