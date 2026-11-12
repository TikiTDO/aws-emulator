// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amazon Managed Workflows for Apache Airflow: the
//! environment an `AWS::MWAA::Environment` declares. One file of [`super`]'s per-type arms,
//! dispatched to by its `Provisioner`.
//!
//! The type's properties ARE `CreateEnvironment`'s own members, named identically on both
//! sides all the way down — Amazon MWAA spells its members in upper camel case, as a
//! CloudFormation property is spelled, and renames none of them
//! ([`MWAA_ENVIRONMENT_MEMBERS`]) — so the evaluated property document is the request, with
//! the two members CloudFormation decides lifted out: the environment's NAME, which that
//! create binds to its URI, and the tag MAP, which carries the tags CloudFormation gives the
//! resource beside the ones the declaration asks for.
//!
//! "Ref returns the name of the Amazon MWAA environment", which is the handle every Amazon
//! MWAA operation on one takes, so the physical id stays the name the naming rules computed.
//! Every other published return value is a value only that service can produce — the ARN it
//! minted, the Apache Airflow web server's URL, the Celery Executor's queue, the two VPC
//! endpoint services and one CloudWatch Logs group ARN per enabled Apache Airflow log type —
//! so each is read off that service's own report of the environment rather than recomputed
//! here ([`reported_environment_attributes`]).
//!
//! A created environment is `CREATING` — "the request to create the environment is in
//! progress", in which it serves no Apache Airflow API — so the resource's creation is held
//! open until Amazon MWAA reports it `AVAILABLE` ([`mwaa_environment_readiness`]), and an
//! accepted update is held open the same way, walking the two phases that service snapshots
//! and then reconfigures the environment in. Its teardown is asynchronous too:
//! `DeleteEnvironment` leaves the environment `DELETING` and the resource's deletion is
//! finished once that service no longer holds it ([`mwaa_environment_removal`]).

use super::*;

/// Amazon MWAA's signing name, from its model (`sigv4Name: airflow`), which speaks
/// `restJson1` — so its calls ride on [`call_rest_json_named`], each on the URI its own
/// operation's `@http` binding declares, naming the operation so a refusal is reported the
/// way that service's own SDK raises it.
const MWAA_SIGNING_NAME: &str = "airflow";

/// The collection Amazon MWAA binds its environments to, as that model's `@http` traits
/// declare: `PUT /environments/{Name}` creates one, `GET /environments/{Name}` reads it,
/// `PATCH /environments/{Name}` changes it and `DELETE /environments/{Name}` removes it.
const MWAA_ENVIRONMENTS_URI: &str = "/environments";

/// The collection its tagging surface is bound to: `POST /tags/{ResourceArn}` adds tags,
/// `DELETE /tags/{ResourceArn}?tagKeys=` removes them and `GET /tags/{ResourceArn}` reads
/// them, each addressing the environment by the ARN that service minted.
const MWAA_TAGS_URI: &str = "/tags";

/// The operations this module calls, as Amazon MWAA's own model names them.
const CREATE_ENVIRONMENT: &str = "CreateEnvironment";
const GET_ENVIRONMENT: &str = "GetEnvironment";
const UPDATE_ENVIRONMENT: &str = "UpdateEnvironment";
const DELETE_ENVIRONMENT: &str = "DeleteEnvironment";
const TAG_RESOURCE: &str = "TagResource";
const UNTAG_RESOURCE: &str = "UntagResource";
const LIST_TAGS_FOR_RESOURCE: &str = "ListTagsForResource";

/// The error code Amazon MWAA answers for an environment it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of its read, its update and its delete).
const MWAA_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The member `GetEnvironment` answers the environment record in.
const ENVIRONMENT_MEMBER: &str = "Environment";

/// The `EnvironmentStatus` an environment serves Apache Airflow in: "the request was
/// successful and the environment is ready to use".
const MWAA_ENVIRONMENT_AVAILABLE: &str = "AVAILABLE";

/// The `EnvironmentStatus` values an environment is still SETTLING in: the ones whose own doc
/// describes a lifecycle change still running, or paused waiting for the caller.
///
/// `CREATING` is where a create starts and `PENDING` where it pauses until the caller's own
/// VPC endpoints exist ("the process to create the environment is paused until you create the
/// required VPC endpoints in your VPC"); `CREATING_SNAPSHOT` and `UPDATING` are the two phases
/// an accepted update walks ("Amazon MWAA is creating a storage volume snapshot of the Amazon
/// RDS database cluster", then "the request to update the environment is in progress"), and
/// `ROLLING_BACK` is where a failed one restores the environment from that snapshot. In none
/// of them has the change the resource is waiting on landed, so each is a state to wait out
/// rather than one to report the resource ready — or failed — in. `DELETING` is not one: a
/// teardown is what [`mwaa_environment_removal`] waits on.
const MWAA_ENVIRONMENT_SETTLING: &[&str] = &[
    "CREATING",
    "CREATING_SNAPSHOT",
    "PENDING",
    "ROLLING_BACK",
    "UPDATING",
];

/// The `EnvironmentStatus` a torn-down environment is reported in while Amazon MWAA is still
/// removing it: "the request to delete the environment is in progress".
const MWAA_ENVIRONMENT_DELETING: &str = "DELETING";

/// The `AWS::MWAA::Environment` properties `CreateEnvironment` takes under its own member
/// name, as `(property, member)` pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against the
/// `CreateEnvironmentInput` shape (`smithy_query(query="operation",
/// operation="CreateEnvironment")` on Amazon MWAA's model). That model spells its members
/// exactly as the type spells its properties — the correspondence holds all the way down, a
/// `NetworkConfiguration`'s `SubnetIds`/`SecurityGroupIds` and a `LoggingConfiguration`'s
/// five `ModuleLoggingConfigurationInput` members (`Enabled`, `LogLevel`) included — so each
/// pair is the same name twice and every declared value is passed through as the template
/// wrote it.
///
/// `Name` is not here because the environment's name is its physical resource id, which the
/// create below binds to its URI; `Tags` is not because the tag set the resource must end up
/// carrying is more than the declaration's own ([`tags_of`]).
const MWAA_ENVIRONMENT_MEMBERS: &[(&str, &str)] = &[
    ("AirflowConfigurationOptions", "AirflowConfigurationOptions"),
    ("AirflowVersion", "AirflowVersion"),
    ("DagS3Path", "DagS3Path"),
    ("EndpointManagement", "EndpointManagement"),
    ("EnvironmentClass", "EnvironmentClass"),
    ("ExecutionRoleArn", "ExecutionRoleArn"),
    ("KmsKey", "KmsKey"),
    ("LoggingConfiguration", "LoggingConfiguration"),
    ("MaxWebservers", "MaxWebservers"),
    ("MaxWorkers", "MaxWorkers"),
    ("MinWebservers", "MinWebservers"),
    ("MinWorkers", "MinWorkers"),
    ("NetworkConfiguration", "NetworkConfiguration"),
    ("PluginsS3ObjectVersion", "PluginsS3ObjectVersion"),
    ("PluginsS3Path", "PluginsS3Path"),
    ("RequirementsS3ObjectVersion", "RequirementsS3ObjectVersion"),
    ("RequirementsS3Path", "RequirementsS3Path"),
    ("Schedulers", "Schedulers"),
    ("SourceBucketArn", "SourceBucketArn"),
    ("StartupScriptS3ObjectVersion", "StartupScriptS3ObjectVersion"),
    ("StartupScriptS3Path", "StartupScriptS3Path"),
    ("WebserverAccessMode", "WebserverAccessMode"),
    ("WeeklyMaintenanceWindowStart", "WeeklyMaintenanceWindowStart"),
];

/// Every `AWS::MWAA::Environment` property this arm maps onto `CreateEnvironment`, so a
/// declared property it does NOT map is refused rather than dropped
/// ([`refuse_unmapped_properties`]): an environment created without what its declaration
/// asked for is not the environment the template declared.
const MWAA_ENVIRONMENT_MAPPED_PROPERTIES: &[&str] = &[
    "AirflowConfigurationOptions",
    "AirflowVersion",
    "DagS3Path",
    "EndpointManagement",
    "EnvironmentClass",
    "ExecutionRoleArn",
    "KmsKey",
    "LoggingConfiguration",
    "MaxWebservers",
    "MaxWorkers",
    "MinWebservers",
    "MinWorkers",
    "Name",
    "NetworkConfiguration",
    "PluginsS3ObjectVersion",
    "PluginsS3Path",
    "RequirementsS3ObjectVersion",
    "RequirementsS3Path",
    "Schedulers",
    "SourceBucketArn",
    "StartupScriptS3ObjectVersion",
    "StartupScriptS3Path",
    "Tags",
    "WebserverAccessMode",
    "WeeklyMaintenanceWindowStart",
];

/// The members of [`MWAA_ENVIRONMENT_MEMBERS`] that `UpdateEnvironment` does NOT take: that
/// call reconfigures a RUNNING environment, so the members that fix an environment at
/// creation are no members of it at all — the `KmsKey` its data is encrypted with and who
/// manages its VPC endpoints (`EndpointManagement`), beside the name itself, which that call
/// addresses the environment by rather than changes.
///
/// Source: `smithy_query(query="operation",operation="UpdateEnvironment")` on that model,
/// whose input carries every other member of the create under the same name.
const MWAA_CREATE_ONLY_MEMBERS: &[&str] = &["EndpointManagement", "KmsKey"];

/// The member of a declared `NetworkConfiguration` that `UpdateEnvironment` takes.
///
/// Its own `NetworkConfiguration` is an `UpdateNetworkConfigurationInput`, whose one member
/// is the security groups the environment's workers run in: the SUBNETS an environment was
/// placed in cannot be changed on it, so a declaration restating them reaches that call
/// without them rather than being sent to a member it has none of.
const MWAA_UPDATABLE_NETWORK_MEMBERS: &[(&str, &str)] = &[("SecurityGroupIds", "SecurityGroupIds")];

/// The property the network placement is declared in, and the member both the create and the
/// update take it as.
const NETWORK_CONFIGURATION_PROPERTY: &str = "NetworkConfiguration";

/// The attributes an `AWS::MWAA::Environment` publishes that Amazon MWAA reports as members
/// of the environment record itself, as `(attribute, member)` pairs.
///
/// Source: the type's own "Return values" section against the `Environment` shape
/// (`smithy_query(query="shape",shape="Environment")`), which reports each under the same
/// name the type publishes it as.
const MWAA_ENVIRONMENT_ATTRIBUTE_MEMBERS: &[(&str, &str)] = &[
    ("Arn", "Arn"),
    ("CeleryExecutorQueue", "CeleryExecutorQueue"),
    ("DatabaseVpcEndpointService", "DatabaseVpcEndpointService"),
    ("WebserverUrl", "WebserverUrl"),
    ("WebserverVpcEndpointService", "WebserverVpcEndpointService"),
];

/// The Apache Airflow log types a `LoggingConfiguration` publishes a CloudWatch Logs group
/// ARN for, as the type's "Return values" section spells each
/// (`LoggingConfiguration.<log type>.CloudWatchLogGroupArn`) and as that service's own
/// `LoggingConfiguration` shape names its members.
const MWAA_LOG_TYPES: &[&str] = &[
    "DagProcessingLogs",
    "SchedulerLogs",
    "TaskLogs",
    "WebserverLogs",
    "WorkerLogs",
];

/// The member the published log-group attribute of one log type is reported in, and the
/// property the five of them hang off.
const LOGGING_CONFIGURATION_MEMBER: &str = "LoggingConfiguration";
const CLOUDWATCH_LOG_GROUP_ARN_MEMBER: &str = "CloudWatchLogGroupArn";

/// The members `GetEnvironment` reports under this type's own property names, as
/// `(property, member)` pairs, for drift detection.
///
/// The scalars of the declaration that service reports back: a property it does not report —
/// and `Tags`, which drift reads nowhere here — is not evidence of a declared value
/// ([`super::drift`]).
const MWAA_ENVIRONMENT_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("AirflowVersion", "AirflowVersion"),
    ("DagS3Path", "DagS3Path"),
    ("EndpointManagement", "EndpointManagement"),
    ("EnvironmentClass", "EnvironmentClass"),
    ("ExecutionRoleArn", "ExecutionRoleArn"),
    ("KmsKey", "KmsKey"),
    ("MaxWebservers", "MaxWebservers"),
    ("MaxWorkers", "MaxWorkers"),
    ("MinWebservers", "MinWebservers"),
    ("MinWorkers", "MinWorkers"),
    ("Name", "Name"),
    ("NetworkConfiguration", "NetworkConfiguration"),
    ("PluginsS3ObjectVersion", "PluginsS3ObjectVersion"),
    ("PluginsS3Path", "PluginsS3Path"),
    ("RequirementsS3ObjectVersion", "RequirementsS3ObjectVersion"),
    ("RequirementsS3Path", "RequirementsS3Path"),
    ("Schedulers", "Schedulers"),
    ("SourceBucketArn", "SourceBucketArn"),
    ("StartupScriptS3ObjectVersion", "StartupScriptS3ObjectVersion"),
    ("StartupScriptS3Path", "StartupScriptS3Path"),
    ("WebserverAccessMode", "WebserverAccessMode"),
    ("WeeklyMaintenanceWindowStart", "WeeklyMaintenanceWindowStart"),
];

/// Create the environment an `AWS::MWAA::Environment` declares, in Amazon MWAA — so a client
/// that deployed the stack reads the environment back through `mwaa get-environment` for the
/// declared name, finds it in `mwaa list-environments`, and reaches the Apache Airflow web
/// server the stack's outputs name, rather than a `ResourceNotFoundException` for an
/// environment nothing created.
///
/// The declaration IS the request ([`MWAA_ENVIRONMENT_MEMBERS`]), under the name that is this
/// resource's physical id, with the declared tags beside the ones CloudFormation gives the
/// resource ([`tags_of`]) as the `TagMap` that call's own `Tags` member takes.
///
/// What that service answers is what the stack reports: the create carries only the ARN it
/// minted, so the rest of the type's published return values — the web server's URL, the
/// Celery Executor's queue, the two VPC endpoint services and each enabled log type's log
/// group — are read off its own report of the environment it has just recorded. The create
/// leaves the environment `CREATING`, and [`mwaa_environment_readiness`] is what the
/// resource's creation is held open on.
pub(super) async fn create_mwaa_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, MWAA_ENVIRONMENT_MAPPED_PROPERTIES)?;
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    copy_members(&properties, MWAA_ENVIRONMENT_MEMBERS, &mut request);
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("Tags".to_string(), Value::Object(tags));
    }
    call_rest_json_named(
        ctx,
        MWAA_SIGNING_NAME,
        region,
        CREATE_ENVIRONMENT,
        "PUT",
        &environment_uri(&resource.physical_resource_id),
        Some(&Value::Object(request)),
    )
    .await?;
    // The create answers the ARN alone, so every published value is read off that service's
    // own report of the environment — one reading for the create and for the readiness read
    // below, so what a stack publishes cannot depend on which call last looked at it.
    let reported = read_mwaa_environment(ctx, region, &resource.physical_resource_id).await?;
    Ok(Created::attributes(reported_environment_attributes(
        reported.as_ref(),
    )))
}

/// Whether Amazon MWAA has finished bringing up — or reconfiguring — the environment a stack
/// created.
///
/// A created environment is `CREATING` ("the request to create the environment is in
/// progress") and one whose VPC endpoints the caller owns is `PENDING` until those endpoints
/// exist, while an accepted update walks `CREATING_SNAPSHOT` and `UPDATING` before it lands;
/// so [`MWAA_ENVIRONMENT_AVAILABLE`] ("the environment is ready to use") is the state the
/// change the resource waits on is finished in, [`MWAA_ENVIRONMENT_SETTLING`] are the states
/// it is still running in, and every other status is one no wait will bring an environment out
/// of: one whose create failed, one whose update failed, one being torn down out of this
/// stack's reach, one left unstable. An environment that service no longer holds is in the
/// same position, nothing being left to create it.
pub(super) async fn mwaa_environment_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(environment) = read_mwaa_environment(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "Amazon MWAA holds no environment {physical_resource_id}"
        )));
    };
    let status = reported_text(&environment, "Status").unwrap_or_default();
    if status == MWAA_ENVIRONMENT_AVAILABLE {
        return Ok(Readiness::Ready);
    }
    if MWAA_ENVIRONMENT_SETTLING.contains(&status.as_str()) {
        return Ok(Readiness::Pending(status));
    }
    Ok(Readiness::Failed(format!(
        "Amazon MWAA reports environment {physical_resource_id} as {status}"
    )))
}

/// The attributes Amazon MWAA publishes for an environment it now reports up: its own read of
/// the environment, read the one way [`reported_environment_attributes`] reads it.
pub(super) async fn mwaa_environment_ready_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    Ok(reported_environment_attributes(
        read_mwaa_environment(ctx, region, physical_resource_id)
            .await?
            .as_ref(),
    ))
}

/// Whether Amazon MWAA has finished removing the environment a stack deleted.
///
/// `DeleteEnvironment` leaves the environment [`MWAA_ENVIRONMENT_DELETING`] — "the request to
/// delete the environment is in progress" — so it is still there when the delete answers, and
/// the deletion is finished once that service no longer holds it.
pub(super) async fn mwaa_environment_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match read_mwaa_environment(ctx, region, physical_resource_id).await? {
        None => Ok(Removal::Gone),
        Some(environment) => Ok(Removal::Pending(
            reported_text(&environment, "Status")
                .unwrap_or_else(|| MWAA_ENVIRONMENT_DELETING.to_string()),
        )),
    }
}

/// Apply a changed declaration to the environment the stack created, through
/// `UpdateEnvironment` — the one call that reconfigures an environment that exists.
///
/// It takes every member of the create but the ones that are fixed when an environment is
/// made ([`MWAA_CREATE_ONLY_MEMBERS`]), and takes the network placement as its security groups
/// alone ([`MWAA_UPDATABLE_NETWORK_MEMBERS`]), so the request is the declaration narrowed to
/// what that call carries; the environment keeps the name it was created under, which is the
/// handle the stack publishes.
///
/// That call is a PATCH — every member of it is optional and merged onto the configuration in
/// force — and Amazon MWAA publishes no call that clears one, so a member the new declaration
/// no longer states keeps the value an earlier deployment gave it rather than being sent as an
/// empty value that service would refuse.
///
/// `Ok(None)` is a change that call cannot make, so the deployment reports it as not applied
/// rather than as a success that dropped it: a changed `Tags` map, which `UpdateEnvironment`
/// does not carry — an environment's tags are that service's own tagging calls
/// ([`retag_mwaa_environment`]).
pub(super) async fn update_mwaa_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let mut request = Map::new();
    if let Some(properties) = declared.as_object() {
        for (property, member) in MWAA_ENVIRONMENT_MEMBERS
            .iter()
            .filter(|(property, _)| !MWAA_CREATE_ONLY_MEMBERS.contains(property))
        {
            let Some(value) = properties.get(*property) else {
                continue;
            };
            let carried = if *property == NETWORK_CONFIGURATION_PROPERTY {
                mapped_members(value, MWAA_UPDATABLE_NETWORK_MEMBERS)
            } else {
                value.clone()
            };
            request.insert((*member).to_string(), carried);
        }
    }
    call_rest_json_named(
        ctx,
        MWAA_SIGNING_NAME,
        region,
        UPDATE_ENVIRONMENT,
        "PATCH",
        &environment_uri(&resource.physical_resource_id),
        Some(&Value::Object(request)),
    )
    .await?;
    // The update answers the ARN alone, exactly as the create does, so what the resource now
    // reports is read off that service's own report of the environment.
    let reported = read_mwaa_environment(ctx, region, &resource.physical_resource_id).await?;
    Ok(Some(Created::attributes(reported_environment_attributes(
        reported.as_ref(),
    ))))
}

/// Bring the tags of the environment a resource record names to the ones this deployment
/// gives it, through Amazon MWAA's own `TagResource` and `UntagResource`.
///
/// The set is the declaration's own `Tags` beside the ones CloudFormation propagates
/// ([`tags_of`]) — the same set the CREATE applied — so a key outside it is one this
/// deployment no longer gives the resource and is removed, and one in it with another value
/// is re-valued. Where the two already agree nothing is called at all, which is what makes
/// this safe to ask on every deployment.
///
/// Those calls address an environment by its ARN, which is the handle that service minted
/// rather than one rebuilt here, so it is read back off the environment itself. An
/// environment Amazon MWAA no longer holds has no tags to reconcile.
pub(super) async fn retag_mwaa_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let Some(arn) = read_mwaa_environment(ctx, region, &resource.physical_resource_id)
        .await?
        .as_ref()
        .and_then(|environment| reported_text(environment, "Arn"))
    else {
        return Ok(());
    };
    let uri = format!("{MWAA_TAGS_URI}/{}", percent_encoded(&arn));
    // That service answers its tag read as the `TagMap` a declaration's `Tags` property is
    // also written as, so one reader serves both sides.
    let held = declared_tags(
        &call_rest_json_named(
            ctx,
            MWAA_SIGNING_NAME,
            region,
            LIST_TAGS_FOR_RESOURCE,
            "GET",
            &uri,
            None,
        )
        .await?,
    );
    let desired = tags_of(declared, propagated);
    let (added, removed) = tag_delta(&held, &desired);
    if !added.is_empty() {
        call_rest_json_named(
            ctx,
            MWAA_SIGNING_NAME,
            region,
            TAG_RESOURCE,
            "POST",
            &uri,
            Some(&Value::Object(
                [("Tags".to_string(), Value::Object(tags_as_map(&added)))]
                    .into_iter()
                    .collect(),
            )),
        )
        .await?;
    }
    if !removed.is_empty() {
        // `tagKeys` is an `@httpQuery` LIST, which travels as the parameter repeated once per
        // key rather than as one joined value.
        let keys = removed
            .iter()
            .map(|key| format!("tagKeys={}", percent_encoded(key)))
            .collect::<Vec<String>>()
            .join("&");
        call_rest_json_named(
            ctx,
            MWAA_SIGNING_NAME,
            region,
            UNTAG_RESOURCE,
            "DELETE",
            &format!("{uri}?{keys}"),
            None,
        )
        .await?;
    }
    Ok(())
}

/// Delete the environment a stack created, addressed by the name Amazon MWAA knows it by —
/// this resource's physical id. An environment that service no longer holds
/// (`ResourceNotFoundException`, judged on the CODE and never on a message that carries the
/// request's own name) is already in the state the delete asks for.
pub(super) async fn delete_mwaa_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_rest_json_named(
        ctx,
        MWAA_SIGNING_NAME,
        region,
        DELETE_ENVIRONMENT,
        "DELETE",
        &environment_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == MWAA_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The environment's configuration as Amazon MWAA reports it, for drift detection:
/// `GetEnvironment`'s own record, read by the name the stack published. One that service no
/// longer holds was deleted.
pub(super) async fn describe_mwaa_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_mwaa_environment(ctx, region, physical_resource_id)
        .await?
        .map(|environment| {
            reported_as_properties(&environment, MWAA_ENVIRONMENT_REPORTED_PROPERTIES)
        }))
}

/// The environment record Amazon MWAA holds under `name`, or `None` when it holds none
/// (`ResourceNotFoundException`, judged on the CODE and never on a message).
async fn read_mwaa_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json_named(
        ctx,
        MWAA_SIGNING_NAME,
        region,
        GET_ENVIRONMENT,
        "GET",
        &environment_uri(name),
        None,
    )
    .await
    {
        Ok(answer) => Ok(answer.get(ENVIRONMENT_MEMBER).cloned()),
        Err(refusal) if refusal.code == MWAA_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The attributes one reported environment gives the resource: the members Amazon MWAA
/// reports under the names the type publishes ([`MWAA_ENVIRONMENT_ATTRIBUTE_MEMBERS`]) plus
/// the CloudWatch Logs group of each Apache Airflow log type it publishes one for.
///
/// An environment that service reports nothing about gives none, and a value it does not
/// report leaves that attribute as the naming rules computed it — a log type that is not
/// enabled is reported with no `CloudWatchLogGroupArn`, there being no group for it.
fn reported_environment_attributes(environment: Option<&Value>) -> BTreeMap<String, String> {
    let Some(environment) = environment else {
        return BTreeMap::new();
    };
    let mut attributes = reported_attributes(environment, MWAA_ENVIRONMENT_ATTRIBUTE_MEMBERS);
    let logging = environment.get(LOGGING_CONFIGURATION_MEMBER);
    for log_type in MWAA_LOG_TYPES {
        let reported = logging
            .and_then(|logging| logging.get(log_type))
            .and_then(|module| reported_text(module, CLOUDWATCH_LOG_GROUP_ARN_MEMBER));
        if let Some(log_group_arn) = reported.filter(|arn| !arn.is_empty()) {
            attributes.insert(
                format!("{LOGGING_CONFIGURATION_MEMBER}.{log_type}.{CLOUDWATCH_LOG_GROUP_ARN_MEMBER}"),
                log_group_arn,
            );
        }
    }
    attributes
}

/// The URI one environment is addressed at: the collection and the `Name` `@httpLabel`
/// escaped as a single path segment ([`percent_encoded`]), so a declared name outside that
/// member's own pattern is refused by Amazon MWAA's own validation rather than addressing
/// another path.
fn environment_uri(name: &str) -> String {
    format!("{MWAA_ENVIRONMENTS_URI}/{}", percent_encoded(name))
}
