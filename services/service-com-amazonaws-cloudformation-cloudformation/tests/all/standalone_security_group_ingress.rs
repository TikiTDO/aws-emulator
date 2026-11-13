// SPDX-License-Identifier: CC0-1.0

//! A standalone CFN ingress must own a real EC2 rule, not a declaration or its group.
//! Uses the existing native multi-service harness, never an acknowledging EC2 callback.
use crate::stack_lifecycle::{
    call, create_stack, ec2, member, router, wait_for_the_transition, xml_blocks, xml_text,
    xml_texts,
};
use serde_json::{Value, json};
use smithy_server_runtime::MultiServiceRouter;

async fn group(router: &MultiServiceRouter, name: &str) -> String {
    let (status, answer) = ec2(
        router,
        "CreateSecurityGroup",
        &[
            ("GroupName", name),
            ("GroupDescription", "external fixture group"),
        ],
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    xml_text(&answer, "groupId").unwrap()
}
async fn sibling(router: &MultiServiceRouter, group: &str) -> String {
    let (status, answer) = ec2(
        router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", group),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", "8080"),
            ("IpPermissions.1.ToPort", "8080"),
            ("IpPermissions.1.IpRanges.1.CidrIp", "192.0.2.0/24"),
        ],
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    xml_text(&answer, "securityGroupRuleId").unwrap()
}
fn template(group: &str, port: i32, description: Option<&str>) -> Value {
    let mut properties = json!({"GroupId":group,"IpProtocol":"tcp","FromPort":port,"ToPort":port,"CidrIp":"10.0.0.17/24"});
    if let Some(description) = description {
        properties["Description"] = json!(description);
    }
    json!({"Resources":{"Ingress":{"Type":"AWS::EC2::SecurityGroupIngress","Properties":properties}},"Outputs":{"Rule":{"Value":{"Ref":"Ingress"}}}})
}
async fn status(router: &MultiServiceRouter, stack: &str) -> String {
    let (code, answer) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(code, 200, "{answer}");
    xml_text(&answer, "StackStatus").unwrap()
}
async fn physical(router: &MultiServiceRouter, stack: &str, logical: &str) -> String {
    let (code, answer) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical),
        ],
    )
    .await;
    assert_eq!(code, 200, "{answer}");
    xml_text(&answer, "PhysicalResourceId").unwrap()
}
async fn rule(router: &MultiServiceRouter, id: &str) -> String {
    let (code, answer) = ec2(
        router,
        "DescribeSecurityGroupRules",
        &[("SecurityGroupRuleId.1", id)],
    )
    .await;
    assert_eq!(code, 200, "{answer}");
    assert_eq!(
        xml_text(&answer, "securityGroupRuleId").as_deref(),
        Some(id)
    );
    assert_eq!(xml_text(&answer, "isEgress").as_deref(), Some("false"));
    answer
}
async fn absent(router: &MultiServiceRouter, id: &str) {
    let (code, answer) = ec2(
        router,
        "DescribeSecurityGroupRules",
        &[("SecurityGroupRuleId.1", id)],
    )
    .await;
    assert_ne!(code, 200, "{answer}");
    assert_eq!(
        xml_text(&answer, "Code").as_deref(),
        Some("InvalidSecurityGroupRuleId.NotFound")
    );
}
async fn ids(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    let (code, answer) = ec2(
        router,
        "DescribeSecurityGroupRules",
        &[("Filter.1.Name", "group-id"), ("Filter.1.Value.1", group)],
    )
    .await;
    assert_eq!(code, 200, "{answer}");
    let mut ids = xml_texts(&answer, "securityGroupRuleId");
    ids.sort();
    ids
}
async fn update(router: &MultiServiceRouter, stack: &str, template: &Value) {
    let (code, answer) = call(
        router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template.to_string()),
        ],
    )
    .await;
    assert_eq!(code, 200, "{answer}");
    wait_for_the_transition().await;
}

#[tokio::test]
async fn standalone_ingress_real_identity_description_replacement_rollback_and_owned_delete() {
    let router = router();
    let target = group(&router, "external-target").await;
    let unrelated = sibling(&router, &target).await;
    let stack = "standalone-ingress-lifecycle";
    let initial = template(&target, 443, None);
    let (code, answer) = create_stack(&router, stack, &initial.to_string(), &[]).await;
    assert_eq!(code, 200, "{answer}");
    let stack_id = xml_text(&answer, "StackId").unwrap();
    wait_for_the_transition().await;
    assert_eq!(status(&router, stack).await, "CREATE_COMPLETE");
    let first = physical(&router, stack, "Ingress").await;
    assert!(first.starts_with("sgr-"));
    let actual = rule(&router, &first).await;
    assert_eq!(
        xml_text(&actual, "groupId").as_deref(),
        Some(target.as_str())
    );
    assert_eq!(xml_text(&actual, "fromPort").as_deref(), Some("443"));
    assert_eq!(
        xml_text(&actual, "cidrIpv4").as_deref(),
        Some("10.0.0.0/24")
    );
    let (_, described) = call(&router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        xml_text(&described, "OutputValue").as_deref(),
        Some(first.as_str())
    );

    update(
        &router,
        stack,
        &template(&target, 443, Some("added description")),
    )
    .await;
    assert_eq!(status(&router, stack).await, "UPDATE_COMPLETE");
    assert_eq!(physical(&router, stack, "Ingress").await, first);
    assert_eq!(
        xml_text(&rule(&router, &first).await, "description").as_deref(),
        Some("added description")
    );
    update(&router, stack, &initial).await;
    assert_eq!(status(&router, stack).await, "UPDATE_COMPLETE");
    assert_eq!(physical(&router, stack, "Ingress").await, first);
    assert!(xml_text(&rule(&router, &first).await, "description").is_none());

    let replaced = template(&target, 8443, None);
    update(&router, stack, &replaced).await;
    assert_eq!(status(&router, stack).await, "UPDATE_COMPLETE");
    let second = physical(&router, stack, "Ingress").await;
    assert_ne!(second, first);
    absent(&router, &first).await;
    assert_eq!(
        xml_text(&rule(&router, &second).await, "fromPort").as_deref(),
        Some("8443")
    );
    rule(&router, &unrelated).await;

    let before = ids(&router, &target).await;
    let mut failed = template(&target, 9443, None);
    failed["Resources"]["LaterFailure"] = json!({"Type":"AWS::EC2::SecurityGroupIngress","DependsOn":"Ingress","Properties":{
        "GroupId":"sg-00000000000000000","IpProtocol":"tcp","FromPort":22,"ToPort":22,"CidrIp":"198.51.100.0/24"
    }});
    update(&router, stack, &failed).await;
    assert_eq!(status(&router, stack).await, "UPDATE_ROLLBACK_COMPLETE");
    assert_eq!(physical(&router, stack, "Ingress").await, second);
    assert_eq!(
        ids(&router, &target).await,
        before,
        "rollback must remove only the new replacement"
    );
    rule(&router, &second).await;
    rule(&router, &unrelated).await;

    // Out-of-band drift changes the permission tuple. Deletion must still use the
    // stored rule ID, not reconstruct and revoke the obsolete declared permission.
    let (code, answer) = ec2(
        &router,
        "ModifySecurityGroupRules",
        &[
            ("GroupId", &target),
            ("SecurityGroupRule.1.SecurityGroupRuleId", &second),
            ("SecurityGroupRule.1.SecurityGroupRule.IpProtocol", "tcp"),
            ("SecurityGroupRule.1.SecurityGroupRule.FromPort", "10043"),
            ("SecurityGroupRule.1.SecurityGroupRule.ToPort", "10043"),
            (
                "SecurityGroupRule.1.SecurityGroupRule.CidrIpv4",
                "10.0.0.0/24",
            ),
        ],
    )
    .await;
    assert_eq!(code, 200, "{answer}");
    let (code, answer) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(code, 200, "{answer}");
    wait_for_the_transition().await;
    assert_eq!(status(&router, &stack_id).await, "DELETE_COMPLETE");
    absent(&router, &second).await;
    rule(&router, &unrelated).await;
    let (code, answer) = ec2(&router, "DescribeSecurityGroups", &[("GroupId.1", &target)]).await;
    assert_eq!(
        code, 200,
        "standalone deletion must not delete the external group: {answer}"
    );
}

#[tokio::test]
async fn standalone_ingress_ref_targets_and_out_of_band_absence() {
    let router = router();
    let stack = "standalone-ingress-refs";
    let template = json!({"Resources":{
        "Target":{"Type":"AWS::EC2::SecurityGroup","Properties":{"GroupDescription":"target"}},
        "Source":{"Type":"AWS::EC2::SecurityGroup","Properties":{"GroupDescription":"source"}},
        "Ingress":{"Type":"AWS::EC2::SecurityGroupIngress","Properties":{
            "GroupId":{"Fn::GetAtt":["Target","GroupId"]},"SourceSecurityGroupId":{"Ref":"Source"},
            "IpProtocol":"tcp","FromPort":5432,"ToPort":5432
        }}
    }});
    let (code, answer) = create_stack(&router, stack, &template.to_string(), &[]).await;
    assert_eq!(code, 200, "{answer}");
    let stack_id = xml_text(&answer, "StackId").unwrap();
    wait_for_the_transition().await;
    assert_eq!(status(&router, stack).await, "CREATE_COMPLETE");
    let target = physical(&router, stack, "Target").await;
    let source = physical(&router, stack, "Source").await;
    let id = physical(&router, stack, "Ingress").await;
    let actual = rule(&router, &id).await;
    let referenced = xml_blocks(&actual, "referencedGroupInfo");
    assert_eq!(referenced.len(), 1);
    assert_eq!(
        xml_text(&referenced[0], "groupId").as_deref(),
        Some(source.as_str())
    );
    let mut changed = template.clone();
    changed["Resources"]["Ingress"]["Properties"]["Description"] =
        json!("source-group description");
    update(&router, stack, &changed).await;
    assert_eq!(status(&router, stack).await, "UPDATE_COMPLETE");
    assert_eq!(physical(&router, stack, "Ingress").await, id);
    let actual = rule(&router, &id).await;
    assert_eq!(
        xml_text(&actual, "description").as_deref(),
        Some("source-group description")
    );
    assert_eq!(
        xml_text(&xml_blocks(&actual, "referencedGroupInfo")[0], "groupId").as_deref(),
        Some(source.as_str())
    );

    let (code, answer) = ec2(
        &router,
        "RevokeSecurityGroupIngress",
        &[("GroupId", &target), ("SecurityGroupRuleId.1", &id)],
    )
    .await;
    assert_eq!(code, 200, "{answer}");
    absent(&router, &id).await;
    let (code, drift) = call(
        &router,
        "DetectStackResourceDrift",
        &[
            member("StackName", stack),
            member("LogicalResourceId", "Ingress"),
        ],
    )
    .await;
    assert_eq!(code, 200, "{drift}");
    assert_eq!(
        xml_text(&drift, "StackResourceDriftStatus").as_deref(),
        Some("DELETED")
    );
    let (code, answer) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(code, 200, "{answer}");
    wait_for_the_transition().await;
    assert_eq!(status(&router, &stack_id).await, "DELETE_COMPLETE");
}

#[tokio::test]
async fn standalone_ingress_invalid_or_duplicate_input_never_adopts_existing_rules() {
    let router = router();
    let target = group(&router, "validation-target").await;
    let unrelated = sibling(&router, &target).await;
    let before = ids(&router, &target).await;
    let mut multiple = template(&target, 443, None);
    multiple["Resources"]["Ingress"]["Properties"]["CidrIpv6"] = json!("2001:db8::/64");
    let mut unknown = template(&target, 443, None);
    unknown["Resources"]["Ingress"]["Properties"]["IgnoredGrant"] = json!(true);
    let mut missing = template(&target, 443, None);
    missing["Resources"]["Ingress"]["Properties"]
        .as_object_mut()
        .unwrap()
        .remove("GroupId");
    let mut duplicate = template(&target, 8080, None);
    duplicate["Resources"]["Ingress"]["Properties"]["CidrIp"] = json!("192.0.2.0/24");
    for (index, template) in [multiple, unknown, missing, duplicate].iter().enumerate() {
        let stack = format!("standalone-ingress-invalid-{index}");
        let (code, _) = create_stack(&router, &stack, &template.to_string(), &[]).await;
        if code == 200 {
            wait_for_the_transition().await;
            assert_ne!(status(&router, &stack).await, "CREATE_COMPLETE");
        }
        assert_eq!(
            ids(&router, &target).await,
            before,
            "invalid declaration must not authorize or adopt a rule"
        );
        rule(&router, &unrelated).await;
    }
}

#[tokio::test]
async fn standalone_ingress_number_parameter_ports_reach_owner_and_delete_by_id() {
    let router = router();
    let target = group(&router, "number-parameter-target").await;
    let unrelated = sibling(&router, &target).await;
    let stack = "standalone-ingress-number-port";
    let mut document = template(&target, 443, None);
    document["Parameters"] = json!({"Port":{"Type":"Number","Default":443}});
    document["Resources"]["Ingress"]["Properties"]["FromPort"] = json!({"Ref":"Port"});
    document["Resources"]["Ingress"]["Properties"]["ToPort"] = json!({"Ref":"Port"});
    let (code, answer) = create_stack(&router, stack, &document.to_string(), &[]).await;
    assert_eq!(code, 200, "{answer}");
    let stack_id = xml_text(&answer, "StackId").unwrap();
    wait_for_the_transition().await;
    assert_eq!(status(&router, stack).await, "CREATE_COMPLETE");
    let id = physical(&router, stack, "Ingress").await;
    assert!(id.starts_with("sgr-"));
    let actual = rule(&router, &id).await;
    assert_eq!(
        xml_text(&actual, "groupId").as_deref(),
        Some(target.as_str())
    );
    assert_eq!(xml_text(&actual, "fromPort").as_deref(), Some("443"));
    assert_eq!(xml_text(&actual, "toPort").as_deref(), Some("443"));
    let (code, described) = call(&router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(code, 200, "{described}");
    assert_eq!(
        xml_text(&described, "OutputValue").as_deref(),
        Some(id.as_str())
    );
    let (code, answer) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(code, 200, "{answer}");
    wait_for_the_transition().await;
    assert_eq!(status(&router, &stack_id).await, "DELETE_COMPLETE");
    absent(&router, &id).await;
    assert_eq!(ids(&router, &target).await, vec![unrelated]);
    let (code, answer) = ec2(&router, "DescribeSecurityGroups", &[("GroupId.1", &target)]).await;
    assert_eq!(
        code, 200,
        "standalone delete must preserve the group: {answer}"
    );
}
