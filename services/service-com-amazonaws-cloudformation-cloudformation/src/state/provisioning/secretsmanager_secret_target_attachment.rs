// SPDX-License-Identifier: CC0-1.0

//! `AWS::SecretsManager::SecretTargetAttachment`: binds an AWS Secrets Manager secret to a
//! database instance or cluster, enriching the secret's JSON value with the target's live
//! connection details and publishing the secret's ARN as the attachment's physical identity.
//!
//! ## Why the attachment updates the secret in Secrets Manager
//!
//! In AWS CloudFormation (and CDK's `DatabaseInstance` / `DatabaseCluster` generated
//! credentials), a stack declares an `AWS::SecretsManager::Secret` containing only the
//! generated `"username"` and `"password"`, declares the database resource consuming that
//! password, and then declares `AWS::SecretsManager::SecretTargetAttachment` referencing both.
//! Downstream stacks and application containers receive `{ Ref: SecretTargetAttachment }` as
//! their database secret ARN and read `"engine"`, `"host"`, `"port"`, `"dbname"`, and
//! `"dbInstanceIdentifier"` (or `"dbClusterIdentifier"`) directly from `GetSecretValue`.
//!
//! Leaving this resource declared-only breaks both halves: `Ref` resolves to a synthetic
//! CloudFormation logical name that `GetSecretValue` rejects with `ResourceNotFoundException`,
//! and the secret itself lacks the database's host and port. Reading the target from its
//! owning service (`DescribeDBInstances`, `DescribeDBClusters`, or `DescribeClusters`) and
//! writing the merged JSON document back via `PutSecretValue` makes both `Ref` and
//! `GetSecretValue` match AWS CloudFormation.

use std::collections::BTreeMap;

use serde_json::{Map, Number, Value};

use super::{
    call_query, call_secretsmanager, xml_scalar_children, Created, Readiness, Refusal, Removal,
};
use crate::state::StoredResource;

/// The resource type this module provisions, shared with [`crate::state::template`].
pub(super) use crate::state::template::SECRETSMANAGER_SECRET_TARGET_ATTACHMENT;

const SECRET_NOT_FOUND: &str = "ResourceNotFoundException";
const NOT_IMPLEMENTED: &str = "NotImplemented";
const INVALID_REQUEST: &str = "InvalidRequest";
const INTERNAL_FAILURE: &str = "InternalFailure";

const SECRET_ID: &str = "SecretId";
const TARGET_ID: &str = "TargetId";
const TARGET_TYPE: &str = "TargetType";
const SECRET_STRING: &str = "SecretString";
const CLIENT_REQUEST_TOKEN: &str = "ClientRequestToken";
const CLIENT_REQUEST_TOKEN_CHARACTERS: usize = 32;
const ID_ATTRIBUTE: &str = "Id";

const RDS_SIGNING_NAME: &str = "rds";
const RDS_API_VERSION: &str = "2014-10-31";
const REDSHIFT_SIGNING_NAME: &str = "redshift";
const REDSHIFT_API_VERSION: &str = "2012-12-01";

/// Every property `AWS::SecretsManager::SecretTargetAttachment` publishes.
const ATTACHMENT_PROPERTIES: &[&str] = &[SECRET_ID, TARGET_ID, TARGET_TYPE];

/// Parsed connection metadata extracted from the target database service.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetConnection {
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    dbname: Option<String>,
    instance_identifier: Option<String>,
    cluster_identifier: Option<String>,
}

/// Attach the secret to the target database by merging the target's live endpoint and
/// identifier fields into the secret's JSON value and returning the secret's ARN.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let arn = attach(ctx, region, &resource.properties, resource).await?;
    let mut attributes = BTreeMap::new();
    attributes.insert(ID_ATTRIBUTE.to_string(), arn.clone());
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// Apply an updated declaration against the same `(SecretId, TargetId, TargetType)` triple.
/// All three properties require replacement when changed (`crate::state::template::replacement_properties`),
/// so an in-place update refreshes the target's connection fields on the same secret.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let arn = attach(ctx, region, declared, resource).await?;
    let mut attributes = BTreeMap::new();
    attributes.insert(ID_ATTRIBUTE.to_string(), arn);
    Ok(Some(Created {
        physical_resource_id: None,
        attributes,
    }))
}

/// `PutSecretValue` stores the updated secret version synchronously, so the attachment is
/// ready as soon as `create` returns.
pub(super) fn readiness() -> Readiness {
    Readiness::Ready
}

/// Removing a `SecretTargetAttachment` detaches the relationship without deleting the
/// underlying `AWS::SecretsManager::Secret` or database resource.
pub(super) fn removal() -> Removal {
    Removal::Gone
}

/// Detaching a target leaves the secret itself intact; the secret's own resource declaration
/// governs its deletion policy.
pub(super) async fn delete(
    _ctx: &smithy_server_runtime::OperationContext<'_>,
    _region: &str,
    _resource: &StoredResource,
) -> Result<(), Refusal> {
    Ok(())
}

/// Drift detection reads no separate attachment record in Secrets Manager.
pub(super) async fn actual_configuration(
    _ctx: &smithy_server_runtime::OperationContext<'_>,
    _region: &str,
    _resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    Ok(None)
}

async fn attach(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    properties: &Value,
    resource: &StoredResource,
) -> Result<String, Refusal> {
    refuse_unapplied_properties(properties, &resource.resource_type)?;
    let secret_id = required_property(properties, SECRET_ID)?;
    let target_id = required_property(properties, TARGET_ID)?;
    let target_type = required_property(properties, TARGET_TYPE)?;

    let (arn, existing_secret_string) = read_secret(ctx, region, &secret_id).await?;
    let target = resolve_target_connection(ctx, region, &target_type, &target_id).await?;
    let updated_secret_string = merge_target_into_secret(existing_secret_string.as_deref(), &target)?;

    let token = crate::state::new_identifier_digits(
        &format!("{SECRETSMANAGER_SECRET_TARGET_ATTACHMENT}/{}", resource.logical_id),
        CLIENT_REQUEST_TOKEN_CHARACTERS,
    );
    let put_request = serde_json::json!({
        SECRET_ID: arn,
        CLIENT_REQUEST_TOKEN: token,
        SECRET_STRING: updated_secret_string,
    });
    call_secretsmanager(ctx, region, "PutSecretValue", &put_request).await?;
    Ok(arn)
}

async fn read_secret(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    secret_id: &str,
) -> Result<(String, Option<String>), Refusal> {
    let request = serde_json::json!({ SECRET_ID: secret_id });
    match call_secretsmanager(ctx, region, "GetSecretValue", &request).await {
        Ok(answered) => {
            let arn = extract_secret_arn(&answered, "GetSecretValue")?;
            let secret_string = answered
                .get(SECRET_STRING)
                .and_then(Value::as_str)
                .map(ToString::to_string);
            Ok((arn, secret_string))
        }
        // A secret created without an initial SecretString/SecretBinary has metadata in
        // DescribeSecret even before its first version is written.
        Err(refusal) if refusal.code == SECRET_NOT_FOUND => {
            let described = call_secretsmanager(ctx, region, "DescribeSecret", &request).await?;
            let arn = extract_secret_arn(&described, "DescribeSecret")?;
            Ok((arn, None))
        }
        Err(refusal) => Err(refusal),
    }
}

fn extract_secret_arn(answered: &Value, operation: &str) -> Result<String, Refusal> {
    answered
        .get("ARN")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| Refusal {
            code: INTERNAL_FAILURE.to_string(),
            message: format!("AWS Secrets Manager answered {operation} without an ARN"),
        })
}

async fn resolve_target_connection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    target_type: &str,
    target_id: &str,
) -> Result<TargetConnection, Refusal> {
    match target_type {
        "AWS::RDS::DBInstance" | "AWS::DocDB::DBInstance" => {
            let parameters = vec![("DBInstanceIdentifier".to_string(), target_id.to_string())];
            let answered = call_query(
                ctx,
                RDS_SIGNING_NAME,
                region,
                RDS_API_VERSION,
                "DescribeDBInstances",
                &parameters,
            )
            .await?;
            Ok(parse_rds_instance_xml(&answered, target_id))
        }
        "AWS::RDS::DBCluster" | "AWS::DocDB::DBCluster" => {
            let parameters = vec![("DBClusterIdentifier".to_string(), target_id.to_string())];
            let answered = call_query(
                ctx,
                RDS_SIGNING_NAME,
                region,
                RDS_API_VERSION,
                "DescribeDBClusters",
                &parameters,
            )
            .await?;
            Ok(parse_rds_cluster_xml(&answered, target_id))
        }
        "AWS::Redshift::Cluster" => {
            let parameters = vec![("ClusterIdentifier".to_string(), target_id.to_string())];
            let answered = call_query(
                ctx,
                REDSHIFT_SIGNING_NAME,
                region,
                REDSHIFT_API_VERSION,
                "DescribeClusters",
                &parameters,
            )
            .await?;
            Ok(parse_redshift_cluster_xml(&answered, target_id))
        }
        other => Err(Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!(
                "{SECRETSMANAGER_SECRET_TARGET_ATTACHMENT} unsupported {TARGET_TYPE}: {other}"
            ),
        }),
    }
}

fn parse_rds_instance_xml(xml: &str, fallback_id: &str) -> TargetConnection {
    let instance = xml_scalar_children(xml, "DBInstance");
    let endpoint = xml_scalar_children(xml, "Endpoint");
    let instance_id = scalar_field(&instance, "DBInstanceIdentifier")
        .unwrap_or_else(|| fallback_id.to_string());
    TargetConnection {
        engine: scalar_field(&instance, "Engine"),
        host: scalar_field(&endpoint, "Address"),
        port: scalar_field(&endpoint, "Port").and_then(|port| port.parse::<u16>().ok()),
        dbname: scalar_field(&instance, "DBName"),
        instance_identifier: Some(instance_id),
        cluster_identifier: scalar_field(&instance, "DBClusterIdentifier"),
    }
}

fn parse_rds_cluster_xml(xml: &str, fallback_id: &str) -> TargetConnection {
    let cluster = xml_scalar_children(xml, "DBCluster");
    let cluster_id = scalar_field(&cluster, "DBClusterIdentifier")
        .unwrap_or_else(|| fallback_id.to_string());
    TargetConnection {
        engine: scalar_field(&cluster, "Engine"),
        host: scalar_field(&cluster, "Endpoint"),
        port: scalar_field(&cluster, "Port").and_then(|port| port.parse::<u16>().ok()),
        dbname: scalar_field(&cluster, "DatabaseName"),
        instance_identifier: None,
        cluster_identifier: Some(cluster_id),
    }
}

fn parse_redshift_cluster_xml(xml: &str, fallback_id: &str) -> TargetConnection {
    let cluster = xml_scalar_children(xml, "Cluster");
    let endpoint = xml_scalar_children(xml, "Endpoint");
    let cluster_id =
        scalar_field(&cluster, "ClusterIdentifier").unwrap_or_else(|| fallback_id.to_string());
    TargetConnection {
        engine: Some("redshift".to_string()),
        host: scalar_field(&endpoint, "Address"),
        port: scalar_field(&endpoint, "Port").and_then(|port| port.parse::<u16>().ok()),
        dbname: scalar_field(&cluster, "DBName"),
        instance_identifier: None,
        cluster_identifier: Some(cluster_id),
    }
}

fn scalar_field(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .map(::service_common::json_wire_values::value_text)
        .filter(|text| !text.is_empty())
}

fn merge_target_into_secret(
    existing_secret_string: Option<&str>,
    target: &TargetConnection,
) -> Result<String, Refusal> {
    let mut object = match existing_secret_string.map(str::trim) {
        None | Some("") => Map::new(),
        Some(raw) => match serde_json::from_str::<Value>(raw) {
            Ok(Value::Object(map)) => map,
            Ok(_) => {
                return Err(Refusal {
                    code: INVALID_REQUEST.to_string(),
                    message: format!(
                        "{SECRETSMANAGER_SECRET_TARGET_ATTACHMENT} requires SecretString to be a JSON object"
                    ),
                });
            }
            Err(error) => {
                return Err(Refusal {
                    code: INVALID_REQUEST.to_string(),
                    message: format!(
                        "{SECRETSMANAGER_SECRET_TARGET_ATTACHMENT} could not parse SecretString as JSON: {error}"
                    ),
                });
            }
        },
    };

    if let Some(engine) = &target.engine {
        object.insert("engine".to_string(), Value::String(engine.clone()));
    }
    if let Some(host) = &target.host {
        object.insert("host".to_string(), Value::String(host.clone()));
    }
    if let Some(port) = target.port {
        object.insert("port".to_string(), Value::Number(Number::from(port)));
    }
    if let Some(dbname) = &target.dbname {
        object.insert("dbname".to_string(), Value::String(dbname.clone()));
    }
    if let Some(instance_id) = &target.instance_identifier {
        object.insert(
            "dbInstanceIdentifier".to_string(),
            Value::String(instance_id.clone()),
        );
    }
    if let Some(cluster_id) = &target.cluster_identifier {
        object.insert(
            "dbClusterIdentifier".to_string(),
            Value::String(cluster_id.clone()),
        );
    }

    serde_json::to_string(&Value::Object(object)).map_err(|error| Refusal {
        code: INTERNAL_FAILURE.to_string(),
        message: error.to_string(),
    })
}

fn required_property(properties: &Value, name: &str) -> Result<String, Refusal> {
    match properties
        .get(name)
        .filter(|value| !value.is_null())
        .map(::service_common::json_wire_values::value_text)
    {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!(
                "{SECRETSMANAGER_SECRET_TARGET_ATTACHMENT} requires the {name} property"
            ),
        }),
    }
}

fn refuse_unapplied_properties(properties: &Value, resource_type: &str) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if ATTACHMENT_PROPERTIES.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {resource_type}"
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_db_instance_connection_fields_preserving_credentials() {
        let initial = r#"{"username":"postgres","password":"s3cr3t-value"}"#;
        let xml = r#"
            <DescribeDBInstancesResponse>
              <DescribeDBInstancesResult>
                <DBInstances>
                  <DBInstance>
                    <DBInstanceIdentifier>app-db-green</DBInstanceIdentifier>
                    <Engine>postgres</Engine>
                    <DBName>appdb</DBName>
                    <Endpoint>
                      <Address>127.0.0.1</Address>
                      <Port>15432</Port>
                    </Endpoint>
                  </DBInstance>
                </DBInstances>
              </DescribeDBInstancesResult>
            </DescribeDBInstancesResponse>
        "#;
        let target = parse_rds_instance_xml(xml, "app-db-green");
        let merged = merge_target_into_secret(Some(initial), &target).expect("merges cleanly");
        let parsed: Value = serde_json::from_str(&merged).expect("valid JSON");
        assert_eq!(parsed["username"], "postgres");
        assert_eq!(parsed["password"], "s3cr3t-value");
        assert_eq!(parsed["engine"], "postgres");
        assert_eq!(parsed["host"], "127.0.0.1");
        assert_eq!(parsed["port"], 15432);
        assert_eq!(parsed["dbname"], "appdb");
        assert_eq!(parsed["dbInstanceIdentifier"], "app-db-green");
    }

    #[test]
    fn parses_rds_cluster_connection_fields() {
        let xml = r#"
            <DescribeDBClustersResponse>
              <DescribeDBClustersResult>
                <DBClusters>
                  <DBCluster>
                    <DBClusterIdentifier>analytics-cluster</DBClusterIdentifier>
                    <Engine>aurora-postgresql</Engine>
                    <DatabaseName>warehouse</DatabaseName>
                    <Endpoint>127.0.0.1</Endpoint>
                    <Port>25432</Port>
                  </DBCluster>
                </DBClusters>
              </DescribeDBClustersResult>
            </DescribeDBClustersResponse>
        "#;
        let target = parse_rds_cluster_xml(xml, "analytics-cluster");
        let merged = merge_target_into_secret(Some(r#"{"username":"admin"}"#), &target)
            .expect("merges cluster fields");
        let parsed: Value = serde_json::from_str(&merged).expect("valid JSON");
        assert_eq!(parsed["username"], "admin");
        assert_eq!(parsed["engine"], "aurora-postgresql");
        assert_eq!(parsed["host"], "127.0.0.1");
        assert_eq!(parsed["port"], 25432);
        assert_eq!(parsed["dbname"], "warehouse");
        assert_eq!(parsed["dbClusterIdentifier"], "analytics-cluster");
    }

    #[test]
    fn refuses_missing_required_or_unknown_properties() {
        let valid = serde_json::json!({
            "SecretId": "arn:aws:secretsmanager:us-east-1:123456789012:secret:db-AbCdEf",
            "TargetId": "app-db",
            "TargetType": "AWS::RDS::DBInstance",
        });
        assert!(refuse_unapplied_properties(&valid, SECRETSMANAGER_SECRET_TARGET_ATTACHMENT).is_ok());
        assert_eq!(
            required_property(&valid, SECRET_ID).unwrap(),
            "arn:aws:secretsmanager:us-east-1:123456789012:secret:db-AbCdEf"
        );

        let unknown = serde_json::json!({
            "SecretId": "db",
            "TargetId": "app-db",
            "TargetType": "AWS::RDS::DBInstance",
            "ExtraField": true,
        });
        let err = refuse_unapplied_properties(&unknown, SECRETSMANAGER_SECRET_TARGET_ATTACHMENT)
            .expect_err("unknown property refused");
        assert!(err.message.contains("ExtraField"));

        let missing = serde_json::json!({ "SecretId": "db" });
        let err = required_property(&missing, TARGET_ID).expect_err("missing TargetId refused");
        assert_eq!(err.code, INVALID_REQUEST);
    }
}
