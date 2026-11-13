// SPDX-License-Identifier: CC0-1.0

//! A completed CFN trail must exist in CloudTrail, not only in the type catalogue.
//! This fixture mounts just CFN, CloudTrail and S3 on one engine. Authentication is
//! explicitly OFF, as in CloudTrail's native harness: these are lifecycle and owner
//! policy witnesses, NOT signature/IAM authorization proofs. All resource changes go
//! over the real in-process APIs; no owner registry is seeded or inspected directly.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::{collections::BTreeMap, sync::Arc};

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, resource_event_reason, wait_for_the_transition,
    xml_blocks, xml_text, REGION,
};

fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(engine),
    );
    router
}

async fn trail(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://cloudtrail.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CloudTrail_20131101.{operation}"))
        // A synthetic scope routes region/service with auth OFF; not a valid signature.
        .header("authorization", format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cloudtrail/aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
        ))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("CloudTrail request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = serde_json::from_slice(response.body()).expect("CloudTrail JSON response");
    (status, body)
}

async fn trail_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, answer) = trail(router, operation, body).await;
    assert_eq!(status, 200, "{operation}: {answer}");
    answer
}

fn template(name: &str, bucket: &str, mut properties: Value, with_policy: bool) -> Value {
    properties["TrailName"] = json!(name);
    properties["S3BucketName"] = json!({"Ref":"Delivery"});
    let mut result = json!({
        "Resources": {
            "Delivery": {"Type":"AWS::S3::Bucket", "DeletionPolicy":"Retain",
                "Properties":{"BucketName":bucket}},
            "Trail": {"Type":"AWS::CloudTrail::Trail", "Properties":properties}
        },
        "Outputs": {
            "TrailRef":{"Value":{"Ref":"Trail"}},
            "TrailArn":{"Value":{"Fn::GetAtt":["Trail","Arn"]}}
        }
    });
    if with_policy {
        // Same actual delivery permissions as CloudTrail's owning fixture, but
        // provisioned through CFN + S3 rather than inserted directly into state.
        result["Resources"]["DeliveryPolicy"] = json!({
            "Type":"AWS::S3::BucketPolicy", "DeletionPolicy":"Retain",
            "Properties":{"Bucket":{"Ref":"Delivery"}, "PolicyDocument":{
                "Version":"2012-10-17", "Statement":[
                    {"Effect":"Allow", "Principal":{"Service":"cloudtrail.amazonaws.com"},
                     "Action":"s3:GetBucketAcl", "Resource":format!("arn:aws:s3:::{bucket}")},
                    {"Effect":"Allow", "Principal":{"Service":"cloudtrail.amazonaws.com"},
                     "Action":"s3:PutObject", "Resource":format!("arn:aws:s3:::{bucket}/*"),
                     "Condition":{"StringEquals":{"s3:x-amz-acl":"bucket-owner-full-control"}}}
                ]
            }}
        });
        result["Resources"]["Trail"]["DependsOn"] = json!("DeliveryPolicy");
    }
    result
}

async fn settled(router: &MultiServiceRouter, stack: &str, expected: &str) -> String {
    wait_for_the_transition().await;
    let (status, body) = describe_stack(router, stack).await;
    assert_eq!(status, 200, "DescribeStacks: {body}");
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(expected),
        "{body}"
    );
    body
}

fn output(body: &str, key: &str) -> String {
    let outputs = xml_blocks(body, "Outputs")
        .into_iter()
        .next()
        .expect("Outputs");
    xml_blocks(&outputs, "member")
        .into_iter()
        .find_map(|entry| {
            (xml_text(&entry, "OutputKey").as_deref() == Some(key))
                .then(|| xml_text(&entry, "OutputValue"))
                .flatten()
        })
        .unwrap_or_else(|| panic!("missing {key}: {body}"))
}

async fn tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let body = trail_ok(router, "ListTags", json!({"ResourceIdList":[arn]})).await;
    let resources = body["ResourceTagList"].as_array().expect("ResourceTagList");
    let resource = resources
        .iter()
        .find(|item| item["ResourceId"] == arn)
        .expect("trail tags");
    resource["TagsList"]
        .as_array()
        .expect("TagsList")
        .iter()
        .map(|item| {
            (
                item["Key"].as_str().expect("tag key").to_owned(),
                item["Value"].as_str().expect("tag value").to_owned(),
            )
        })
        .collect()
}

async fn update(router: &MultiServiceRouter, stack: &str, template: &Value) {
    let (status, body) = call(
        router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template.to_string()),
        ],
    )
    .await;
    assert_eq!(status, 200, "UpdateStack: {body}");
    settled(router, stack, "UPDATE_COMPLETE").await;
}

async fn absent(router: &MultiServiceRouter, name: &str) {
    let (status, body) = trail(router, "GetTrail", json!({"Name":name})).await;
    assert_eq!(status, 400, "GetTrail absent: {body}");
    assert_eq!(
        body["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next(),
        Some("TrailNotFoundException"),
        "{body}"
    );
}

async fn s3(router: &MultiServiceRouter, method: &str, bucket: &str, body: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://s3.{REGION}.amazonaws.com/{bucket}/preserved-log"
        ))
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("S3 request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

#[tokio::test]
async fn trail_stack_creates_updates_clears_and_deletes_the_actual_owner() {
    let router = router();
    let name = "cfn-trail-lifecycle";
    let bucket = "cfn-trail-lifecycle-delivery";
    let stack = "trail-lifecycle";
    let initial = template(
        name,
        bucket,
        json!({
            "IsLogging":true, "S3KeyPrefix":"initial", "EnableLogFileValidation":true,
            "IncludeGlobalServiceEvents":false, "IsMultiRegionTrail":true,
            "EventSelectors":[{"ReadWriteType":"WriteOnly", "IncludeManagementEvents":true}],
            "InsightSelectors":[{"InsightType":"ApiCallRateInsight"}],
        "AggregationConfigurations":[{"EventCategory":"Data","Templates":["API_ACTIVITY"]}],
            "Tags":[{"Key":"phase","Value":"initial"},{"Key":"remove-me","Value":"yes"}]
        }),
        true,
    );
    let (status, body) = create_stack(
        &router,
        stack,
        &initial.to_string(),
        &[
            member("Tags.member.1.Key", "stack-tag"),
            member("Tags.member.1.Value", "propagated"),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateStack: {body}");
    let body = settled(&router, stack, "CREATE_COMPLETE").await;
    assert_eq!(output(&body, "TrailRef"), name);
    let arn = output(&body, "TrailArn");
    let owner = trail_ok(&router, "GetTrail", json!({"Name":name})).await;
    assert_eq!(owner["Trail"]["Name"], name);
    assert_eq!(owner["Trail"]["TrailARN"], arn);
    assert_eq!(owner["Trail"]["S3BucketName"], bucket);
    assert_eq!(owner["Trail"]["S3KeyPrefix"], "initial");
    assert_eq!(owner["Trail"]["LogFileValidationEnabled"], true);
    assert_eq!(owner["Trail"]["IsMultiRegionTrail"], true);
    assert_eq!(owner["Trail"]["IncludeGlobalServiceEvents"], false);
    assert_eq!(
        trail_ok(&router, "GetTrailStatus", json!({"Name":name})).await["IsLogging"],
        true
    );
    assert_eq!(
        trail_ok(&router, "GetEventSelectors", json!({"TrailName":name})).await["EventSelectors"]
            [0]["ReadWriteType"],
        "WriteOnly"
    );
    assert_eq!(
        trail_ok(&router, "GetInsightSelectors", json!({"TrailName":name})).await
            ["InsightSelectors"][0]["InsightType"],
        "ApiCallRateInsight"
    );
    assert_eq!(
        trail_ok(&router, "GetEventConfiguration", json!({"TrailName":name})).await
            ["AggregationConfigurations"],
        json!([{"EventCategory":"Data","Templates":["API_ACTIVITY"]}])
    );
    let initial_tags = tags(&router, &arn).await;
    assert_eq!(
        initial_tags.get("phase").map(String::as_str),
        Some("initial")
    );
    assert_eq!(
        initial_tags.get("stack-tag").map(String::as_str),
        Some("propagated")
    );
    trail_ok(
        &router,
        "AddTags",
        json!({"ResourceId":arn,
        "TagsList":[{"Key":"external","Value":"keep"}]}),
    )
    .await;

    let changed = template(
        name,
        bucket,
        json!({
            "IsLogging":false, "S3KeyPrefix":"updated", "EnableLogFileValidation":false,
            "AdvancedEventSelectors":[{"Name":"management", "FieldSelectors":[
                {"Field":"eventCategory","Equals":["Management"]}]}],
            "Tags":[{"Key":"phase","Value":"updated"}]
        }),
        true,
    );
    update(&router, stack, &changed).await;
    let owner = trail_ok(&router, "GetTrail", json!({"Name":name})).await;
    assert_eq!(owner["Trail"]["TrailARN"], arn);
    assert_eq!(owner["Trail"]["S3KeyPrefix"], "updated");
    assert_eq!(owner["Trail"]["LogFileValidationEnabled"], false);
    assert_eq!(owner["Trail"]["IsMultiRegionTrail"], false);
    assert_eq!(owner["Trail"]["IncludeGlobalServiceEvents"], true);
    assert_eq!(
        trail_ok(&router, "GetTrailStatus", json!({"Name":name})).await["IsLogging"],
        false
    );
    assert_eq!(
        trail_ok(&router, "GetEventSelectors", json!({"TrailName":name})).await
            ["AdvancedEventSelectors"][0]["Name"],
        "management"
    );
    let changed_tags = tags(&router, &arn).await;
    assert_eq!(
        changed_tags.get("phase").map(String::as_str),
        Some("updated")
    );
    assert!(!changed_tags.contains_key("remove-me"));
    assert_eq!(
        changed_tags.get("external").map(String::as_str),
        Some("keep")
    );

    update(
        &router,
        stack,
        &template(name, bucket, json!({"IsLogging":true}), true),
    )
    .await;
    let owner = trail_ok(&router, "GetTrail", json!({"Name":name})).await;
    assert!(
        owner["Trail"]
            .get("S3KeyPrefix")
            .is_none_or(|v| v.is_null() || v == ""),
        "{owner}"
    );
    assert_eq!(
        trail_ok(&router, "GetTrailStatus", json!({"Name":name})).await["IsLogging"],
        true
    );
    let selectors = trail_ok(&router, "GetEventSelectors", json!({"TrailName":name})).await;
    assert_eq!(
        selectors["EventSelectors"]
            .as_array()
            .expect("default selector")
            .len(),
        1
    );
    assert_eq!(selectors["EventSelectors"][0]["ReadWriteType"], "All");
    assert_eq!(
        selectors["EventSelectors"][0]["IncludeManagementEvents"],
        true
    );
    assert!(selectors
        .get("AdvancedEventSelectors")
        .is_none_or(|v| v.is_null() || v == &json!([])));
    // The owner reports disabled Insights as its modeled exception, not an empty success.
    let (status, insights) = trail(&router, "GetInsightSelectors", json!({"TrailName":name})).await;
    assert_eq!(status, 400, "{insights}");
    assert_eq!(
        insights["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next(),
        Some("InsightNotEnabledException"),
        "{insights}"
    );
    let aggregation = trail_ok(&router, "GetEventConfiguration", json!({"TrailName":name})).await;
    assert!(
        aggregation
            .get("AggregationConfigurations")
            .is_none_or(|v| v.is_null() || v == &json!([])),
        "removed aggregation must not survive: {aggregation}"
    );
    assert!(!tags(&router, &arn).await.contains_key("phase"));
    let (status, answer) = s3(&router, "PUT", bucket, "existing-log-data").await;
    assert_eq!(status, 200, "S3 PutObject: {answer}");
    let (status, answer) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(status, 200, "DeleteStack: {answer}");
    wait_for_the_transition().await;
    // Trigger the same lazy CFN reconciliation as the rest of the native harness.
    describe_stack(&router, stack).await;
    absent(&router, name).await;
    assert_eq!(
        s3(&router, "GET", bucket, "").await,
        (200, "existing-log-data".into())
    );
    let successor = trail_ok(
        &router,
        "CreateTrail",
        json!({"Name":name,"S3BucketName":bucket}),
    )
    .await;
    assert_eq!(
        successor["Name"], name,
        "deletion must release the owner name"
    );
}

#[tokio::test]
async fn missing_delivery_policy_is_an_owner_refusal_not_create_complete() {
    let router = router();
    let name = "cfn-trail-no-policy";
    let document = template(
        name,
        "cfn-trail-no-policy-delivery",
        json!({"IsLogging":false}),
        false,
    );
    let (status, body) = create_stack(
        &router,
        "trail-no-policy",
        &document.to_string(),
        &[member("DisableRollback", "true")],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    settled(&router, "trail-no-policy", "CREATE_FAILED").await;
    let reason = resource_event_reason(&router, "trail-no-policy", "Trail", "CREATE_FAILED").await;
    assert!(
        reason.contains("InsufficientS3BucketPolicyException"),
        "{reason}"
    );
    absent(&router, name).await;
}

fn rejected_selectors() -> Value {
    // Structurally valid; the actual owning API rejects selecting no events.
    json!([{"IncludeManagementEvents":false, "DataResources":[]}])
}

#[tokio::test]
async fn post_create_selector_refusal_cleans_up_even_when_stack_rollback_is_disabled() {
    let router = router();
    let name = "cfn-trail-create-cleanup";
    let document = template(
        name,
        "cfn-trail-create-cleanup-delivery",
        json!({
            "IsLogging":true, "EventSelectors":rejected_selectors()
        }),
        true,
    );
    let (status, body) = create_stack(
        &router,
        "trail-create-cleanup",
        &document.to_string(),
        &[member("DisableRollback", "true")],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    settled(&router, "trail-create-cleanup", "CREATE_FAILED").await;
    let reason =
        resource_event_reason(&router, "trail-create-cleanup", "Trail", "CREATE_FAILED").await;
    assert!(
        reason.contains("InvalidEventSelectorsException"),
        "{reason}"
    );
    absent(&router, name).await;
}

// Standard preserve-successful-resources rolls FAILED resources back to their last
// stable deployment; drift-aware changesets instead retain actual pre-deployment state.
// https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/stack-failure-options.html
// https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/drift-aware-change-sets.html
#[tokio::test]
async fn failed_standard_update_restores_last_stable_declared_configuration() {
    failed_update(false).await;
}

#[tokio::test]
async fn failed_drift_aware_change_set_restores_actual_owner_preimage() {
    failed_update(true).await;
}

async fn failed_update(drift_aware: bool) {
    let router = router();
    let name = "cfn-trail-update-compensation";
    let bucket = "cfn-trail-update-compensation-delivery";
    let stack = "trail-update-compensation";
    let initial = template(
        name,
        bucket,
        json!({"IsLogging":true,"S3KeyPrefix":"declared"}),
        true,
    );
    let (status, body) = create_stack(&router, stack, &initial.to_string(), &[]).await;
    assert_eq!(status, 200, "{body}");
    settled(&router, stack, "CREATE_COMPLETE").await;
    let declared_before = trail_ok(&router, "GetTrail", json!({"Name":name})).await;
    trail_ok(
        &router,
        "UpdateTrail",
        json!({"Name":name,"S3KeyPrefix":"actual-before-update"}),
    )
    .await;
    let actual_before = trail_ok(&router, "GetTrail", json!({"Name":name})).await;
    let arn = actual_before["Trail"]["TrailARN"]
        .as_str()
        .expect("owner trail ARN")
        .to_owned();
    if drift_aware {
        // Actual-state rollback must restore this value without adopting its key
        // into CFN's managed-tag set. Neither template declares externalU.
        trail_ok(
            &router,
            "AddTags",
            json!({
                "ResourceId":arn, "TagsList":[{"Key":"externalU","Value":"outside-cfn"}]
            }),
        )
        .await;
        assert_eq!(
            tags(&router, &arn)
                .await
                .get("externalU")
                .map(String::as_str),
            Some("outside-cfn")
        );
    }
    let before = if drift_aware {
        actual_before
    } else {
        declared_before
    };
    let selectors_before = trail_ok(&router, "GetEventSelectors", json!({"TrailName":name})).await;
    let changed = template(
        name,
        bucket,
        json!({
            "IsLogging":false, "S3KeyPrefix":"must-not-survive", "EnableLogFileValidation":true,
            "EventSelectors":rejected_selectors()
        }),
        true,
    );
    let (status, body) = if drift_aware {
        let (status, body) = call(
            &router,
            "CreateChangeSet",
            &[
                member("StackName", stack),
                member("ChangeSetName", "restore-actual"),
                member("ChangeSetType", "UPDATE"),
                member("DeploymentMode", "REVERT_DRIFT"),
                member("TemplateBody", &changed.to_string()),
            ],
        )
        .await;
        assert_eq!(status, 200, "CreateChangeSet: {body}");
        wait_for_the_transition().await;
        let (status, body) = call(
            &router,
            "DescribeChangeSet",
            &[
                member("StackName", stack),
                member("ChangeSetName", "restore-actual"),
            ],
        )
        .await;
        assert_eq!(status, 200, "DescribeChangeSet: {body}");
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some("CREATE_COMPLETE"),
            "{body}"
        );
        assert_eq!(
            xml_text(&body, "DeploymentMode").as_deref(),
            Some("REVERT_DRIFT"),
            "{body}"
        );
        call(
            &router,
            "ExecuteChangeSet",
            &[
                member("StackName", stack),
                member("ChangeSetName", "restore-actual"),
                member("DisableRollback", "true"),
            ],
        )
        .await
    } else {
        call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack),
                member("TemplateBody", &changed.to_string()),
                member("DisableRollback", "true"),
            ],
        )
        .await
    };
    assert_eq!(status, 200, "{body}");
    settled(&router, stack, "UPDATE_FAILED").await;
    let reason = resource_event_reason(&router, stack, "Trail", "UPDATE_FAILED").await;
    assert!(
        reason.contains("InvalidEventSelectorsException"),
        "{reason}"
    );
    assert_eq!(
        trail_ok(&router, "GetTrail", json!({"Name":name})).await,
        before,
        "rollback baseline must follow the selected deployment contract (drift_aware={drift_aware})"
    );
    assert_eq!(
        trail_ok(&router, "GetEventSelectors", json!({"TrailName":name})).await,
        selectors_before
    );
    assert_eq!(
        trail_ok(&router, "GetTrailStatus", json!({"Name":name})).await["IsLogging"],
        true
    );
    if drift_aware {
        assert_eq!(
            tags(&router, &arn)
                .await
                .get("externalU")
                .map(String::as_str),
            Some("outside-cfn"),
            "rollback must preserve the external tag"
        );
        let next = template(
            name,
            bucket,
            json!({
                "IsLogging":true, "S3KeyPrefix":"successful-after-rollback"
            }),
            true,
        );
        // This is an ordinary UpdateStack, not another drift-aware execution.
        // UPDATE_FAILED requires an explicit preserve-successful-resources choice
        // to fix forward; it does not change this update's tag ownership contract.
        let (status, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack),
                member("TemplateBody", &next.to_string()),
                member("DisableRollback", "true"),
            ],
        )
        .await;
        assert_eq!(status, 200, "fix-forward UpdateStack: {body}");
        settled(&router, stack, "UPDATE_COMPLETE").await;
        assert_eq!(
            trail_ok(&router, "GetTrail", json!({"Name":name})).await["Trail"]["S3KeyPrefix"],
            "successful-after-rollback"
        );
        assert_eq!(
            tags(&router, &arn)
                .await
                .get("externalU")
                .map(String::as_str),
            Some("outside-cfn"),
            "rollback must not claim externalU and delete it on the next ordinary update"
        );
    }
}
