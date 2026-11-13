// SPDX-License-Identifier: CC0-1.0

//! A standalone ingress resource owns one EC2-minted rule, never its whole group.
//! All effects use the existing signed in-process query owner. Description updates use
//! the implemented ModifySecurityGroupRules; the dedicated description operation is a stub.

use super::{Created, Refusal, call_query, xml_element, xml_elements, xml_scalar_children};
use crate::state::StoredResource;
use crate::state::provisioner::{
    EC2_API_VERSION, EC2_SECURITY_GROUP_INGRESS_MEMBERS, EC2_SIGNING_NAME, ec2_reported_entry,
    security_group_rule_parameters,
};
use serde_json::{Map, Value};
use smithy_server_runtime::OperationContext;
use std::collections::BTreeMap;

pub(in crate::state) const SECURITY_GROUP_INGRESS: &str = "AWS::EC2::SecurityGroupIngress";
const MISSING_RULE: &str = "InvalidSecurityGroupRuleId.NotFound";
const MISSING_GROUP: &str = "InvalidGroup.NotFound";
pub(in crate::state) const REPLACEMENT_PROPERTIES: &[&str] = &[
    "CidrIp",
    "CidrIpv6",
    "FromPort",
    "GroupId",
    "GroupName",
    "IpProtocol",
    "SourcePrefixListId",
    "SourceSecurityGroupId",
    "SourceSecurityGroupName",
    "SourceSecurityGroupOwnerId",
    "ToPort",
];

fn invalid(message: &str) -> Refusal {
    Refusal {
        code: "InvalidRequest".into(),
        message: format!("{SECURITY_GROUP_INGRESS}: {message}"),
    }
}
fn malformed() -> Refusal {
    Refusal {
        code: "InternalFailure".into(),
        message: "EC2 returned no unambiguous inbound rule identity/configuration".into(),
    }
}
fn field<'a>(map: &'a Map<String, Value>, name: &str) -> Result<&'a str, Refusal> {
    map.get(name)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(malformed)
}

/// One target and one grant are checked before the authorize: EC2 permits a multi-grant
/// IpPermission, but one standalone CFN resource must not acquire several rule identities.
fn parameters(properties: &Value) -> Result<Vec<(String, String)>, Refusal> {
    let object = properties
        .as_object()
        .ok_or_else(|| invalid("Properties must be an object"))?;
    for (name, value) in object {
        if name != "Description" && !REPLACEMENT_PROPERTIES.contains(&name.as_str()) {
            return Err(invalid(&format!("unsupported property {name}")));
        }
        if name == "FromPort" || name == "ToPort" {
            if value.as_i64().and_then(|n| i32::try_from(n).ok()).is_none() {
                return Err(invalid("ports must be signed 32-bit integers"));
            }
        } else if value.as_str().is_none() || (name != "Description" && value.as_str() == Some(""))
        {
            return Err(invalid(
                "rule selectors and protocol must be nonempty strings",
            ));
        }
    }
    let count = |names: &[&str]| {
        names
            .iter()
            .filter(|name| object.contains_key(**name))
            .count()
    };
    if count(&["GroupId", "GroupName"]) != 1 {
        return Err(invalid("exactly one GroupId or GroupName is required"));
    }
    if !object.contains_key("IpProtocol") {
        return Err(invalid("IpProtocol is required"));
    }
    let group_sources = count(&["SourceSecurityGroupId", "SourceSecurityGroupName"]);
    if group_sources > 1
        || count(&["CidrIp", "CidrIpv6", "SourcePrefixListId"]) + group_sources != 1
    {
        return Err(invalid("exactly one source is required"));
    }
    if object.contains_key("SourceSecurityGroupOwnerId") && group_sources != 1 {
        return Err(invalid(
            "SourceSecurityGroupOwnerId requires a source group",
        ));
    }
    let mut rule = object.clone();
    let mut result = Vec::new();
    for target in ["GroupId", "GroupName"] {
        if let Some(Value::String(value)) = rule.remove(target) {
            result.push((target.into(), value));
        }
    }
    result.extend(security_group_rule_parameters(
        &Value::Object(rule),
        EC2_SECURITY_GROUP_INGRESS_MEMBERS,
    )?);
    Ok(result)
}

struct Rule {
    fields: Map<String, Value>,
    referenced: Map<String, Value>,
}
impl Rule {
    fn id(&self) -> Result<&str, Refusal> {
        field(&self.fields, "SecurityGroupRuleId")
    }
    fn group(&self) -> Result<&str, Refusal> {
        field(&self.fields, "GroupId")
    }
}
fn parse_rule(document: &str, expected: Option<&str>) -> Result<Rule, Refusal> {
    let ids = xml_elements(document, "securityGroupRuleId");
    if ids.len() != 1 || expected.is_some_and(|id| ids[0] != id) {
        return Err(malformed());
    }
    let fields = ec2_reported_entry(document, "securityGroupRuleSet");
    let rule = Rule {
        fields,
        referenced: xml_scalar_children(document, "referencedGroupInfo"),
    };
    if rule.id()? != ids[0]
        || !rule.id()?.starts_with("sgr-")
        || field(&rule.fields, "IsEgress")? != "false"
    {
        return Err(malformed());
    }
    rule.group()?;
    field(&rule.fields, "IpProtocol")?;
    let sources = ["CidrIpv4", "CidrIpv6", "PrefixListId"]
        .iter()
        .filter(|name| rule.fields.contains_key(**name))
        .count()
        + usize::from(rule.referenced.contains_key("groupId"));
    if sources != 1 {
        return Err(malformed());
    }
    Ok(rule)
}
async fn described(
    ctx: &OperationContext<'_>,
    region: &str,
    id: &str,
) -> Result<Option<Rule>, Refusal> {
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeSecurityGroupRules",
        &[("SecurityGroupRuleId.1".into(), id.into())],
    )
    .await
    {
        Ok(document) => parse_rule(&document, Some(id)).map(Some),
        Err(refusal) if refusal.code == MISSING_RULE => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

pub(super) async fn create(
    ctx: &OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let request = parameters(&resource.properties)?;
    let answer = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "AuthorizeSecurityGroupIngress",
        &request,
    )
    .await?;
    if xml_element(&answer, "return").as_deref() != Some("true") {
        return Err(malformed());
    }
    let rule = parse_rule(&answer, None)?;
    if resource
        .properties
        .get("GroupId")
        .and_then(Value::as_str)
        .is_some_and(|id| rule.group().ok() != Some(id))
    {
        return Err(malformed());
    }
    Ok(Created {
        physical_resource_id: Some(rule.id()?.into()),
        attributes: BTreeMap::new(),
    })
}

pub(super) async fn delete(
    ctx: &OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let Some(rule) = described(ctx, region, &resource.physical_resource_id).await? else {
        return Ok(());
    };
    let request = vec![
        ("GroupId".into(), rule.group()?.into()),
        ("SecurityGroupRuleId.1".into(), rule.id()?.into()),
    ];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "RevokeSecurityGroupIngress",
        &request,
    )
    .await
    {
        Ok(answer) if xml_element(&answer, "return").as_deref() == Some("true") => Ok(()),
        Ok(_) => Err(malformed()),
        Err(refusal) if refusal.code == MISSING_RULE || refusal.code == MISSING_GROUP => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

pub(super) async fn update(
    ctx: &OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    parameters(declared)?;
    if REPLACEMENT_PROPERTIES
        .iter()
        .any(|name| resource.properties.get(*name) != declared.get(*name))
    {
        return Err(invalid(
            "non-description changes require resource replacement",
        ));
    }
    let rule = described(ctx, region, &resource.physical_resource_id)
        .await?
        .ok_or_else(|| Refusal {
            code: MISSING_RULE.into(),
            message: "the ingress rule to update no longer exists".into(),
        })?;
    let prefix = "SecurityGroupRule.1.SecurityGroupRule";
    let mut request = vec![
        ("GroupId".into(), rule.group()?.into()),
        (
            "SecurityGroupRule.1.SecurityGroupRuleId".into(),
            rule.id()?.into(),
        ),
    ];
    for name in [
        "IpProtocol",
        "FromPort",
        "ToPort",
        "CidrIpv4",
        "CidrIpv6",
        "PrefixListId",
    ] {
        if let Some(value) = rule.fields.get(name).and_then(Value::as_str) {
            request.push((format!("{prefix}.{name}"), value.into()));
        }
    }
    if let Some(id) = rule.referenced.get("groupId").and_then(Value::as_str) {
        request.push((format!("{prefix}.ReferencedGroupId"), id.into()));
    }
    if let Some(description) = declared.get("Description").and_then(Value::as_str) {
        request.push((format!("{prefix}.Description"), description.into()));
    }
    let answer = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "ModifySecurityGroupRules",
        &request,
    )
    .await?;
    if xml_element(&answer, "return").as_deref() != Some("true") {
        return Err(malformed());
    }
    Ok(Created {
        physical_resource_id: Some(resource.physical_resource_id.clone()),
        attributes: BTreeMap::new(),
    })
}

async fn group_name(ctx: &OperationContext<'_>, region: &str, id: &str) -> Result<Value, Refusal> {
    let answer = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeSecurityGroups",
        &[("GroupId.1".into(), id.into())],
    )
    .await?;
    let group = ec2_reported_entry(&answer, "securityGroupInfo");
    if field(&group, "GroupId")? != id {
        return Err(malformed());
    }
    Ok(Value::String(field(&group, "GroupName")?.into()))
}

pub(super) async fn actual_configuration(
    ctx: &OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let Some(rule) = described(ctx, region, &resource.physical_resource_id).await? else {
        return Ok(None);
    };
    let mut actual = Map::new();
    if resource.properties.get("GroupName").is_some() {
        actual.insert(
            "GroupName".into(),
            group_name(ctx, region, rule.group()?).await?,
        );
    } else {
        actual.insert("GroupId".into(), Value::String(rule.group()?.into()));
    }
    for (native, property) in [
        ("IpProtocol", "IpProtocol"),
        ("Description", "Description"),
        ("CidrIpv4", "CidrIp"),
        ("CidrIpv6", "CidrIpv6"),
        ("PrefixListId", "SourcePrefixListId"),
    ] {
        if let Some(value) = rule.fields.get(native) {
            actual.insert(property.into(), value.clone());
        }
    }
    for port in ["FromPort", "ToPort"] {
        if let Some(value) = rule.fields.get(port) {
            let number = value
                .as_str()
                .and_then(|text| text.parse::<i32>().ok())
                .ok_or_else(malformed)?;
            actual.insert(port.into(), Value::from(number));
        }
    }
    if let Some(id) = rule.referenced.get("groupId").and_then(Value::as_str) {
        if resource.properties.get("SourceSecurityGroupName").is_some() {
            actual.insert(
                "SourceSecurityGroupName".into(),
                group_name(ctx, region, id).await?,
            );
        } else {
            actual.insert("SourceSecurityGroupId".into(), Value::String(id.into()));
        }
        if resource
            .properties
            .get("SourceSecurityGroupOwnerId")
            .is_some()
        {
            actual.insert(
                "SourceSecurityGroupOwnerId".into(),
                Value::String(field(&rule.referenced, "userId")?.into()),
            );
        }
    }
    Ok(Some(Value::Object(actual)))
}
