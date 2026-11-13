// SPDX-License-Identifier: CC0-1.0

//! The seam into this emulator's own CloudFormation: the template a product version
//! NAMES, and the change set a plan IS.
//!
//! EVERY call here is dispatched IN PROCESS. The client is built on the framework's
//! callback connector ([`in_process_cloudformation`]), which routes a request to the
//! service mounted beside this one inside this same process: no socket is opened, no
//! host name is resolved, nothing outside this process is reachable, and no value a
//! request carries can make this process connect anywhere. The endpoint URL and the
//! Region on that client are addressing for the in-process transport only.
//!
//! Neither of the two things asked for is Service Catalog's to compute. A provisioning
//! artifact does not hold its template — it names one, by URL or by the stack that
//! contains it — and a plan "includes the list of resources to be created (when
//! provisioning a new product) or modified (when updating a provisioned product) when
//! the plan is executed", which is a CloudFormation change set over that template. So
//! both are asked for, and the resource changes a plan reports are the ones
//! CloudFormation computed rather than any this service derived.

use std::collections::BTreeMap;

use crate::ops::types::{
    ResourceChange, ResourceChangeDetail, ResourceTargetDefinition, UpdateProvisioningParameter,
};

/// Where the template of one provisioning artifact is, as that version's `Info` names
/// it: "specify the template source with one of the following options, but not both.
/// Keys accepted: [ LoadTemplateFromURL, ImportFromPhysicalId ]."
///
/// Both are addresses rather than bodies, and CloudFormation is what resolves either —
/// a URL names an object in this account's object store, and a physical id names a
/// CloudFormation stack ("currently supports CloudFormation stack ARN") whose own
/// template is the version's.
#[derive(Debug, Clone)]
pub enum TemplateSource {
    /// `LoadTemplateFromURL`: "the URL of the CloudFormation template in Amazon S3 or
    /// GitHub in JSON format".
    Url(String),
    /// `ImportFromPhysicalId`: "the physical id of the resource that contains the
    /// template".
    Stack(String),
    /// A version whose `Info` names neither, so no template is addressed at all.
    Unaddressed,
}

/// The `Info` key naming a template to read from a URL
/// (`ProvisioningArtifactProperties$Info`: "Keys accepted: [ `LoadTemplateFromURL`,
/// `ImportFromPhysicalId` ]").
pub const LOAD_TEMPLATE_FROM_URL: &str = "LoadTemplateFromURL";

/// The `Info` key naming the physical id of a resource that contains the template.
pub const IMPORT_FROM_PHYSICAL_ID: &str = "ImportFromPhysicalId";

/// Where the version's template is, read from the `Info` map the version was created
/// with ([`TemplateSource`]). A map naming both keys is not one `CreateProduct` stores
/// — it refuses that request — so the URL is read first and the physical id answers for
/// a version that names one instead.
pub fn template_source(info: &BTreeMap<String, String>) -> TemplateSource {
    if let Some(url) = info
        .get(LOAD_TEMPLATE_FROM_URL)
        .filter(|url| !url.is_empty())
    {
        return TemplateSource::Url(url.clone());
    }
    match info
        .get(IMPORT_FROM_PHYSICAL_ID)
        .filter(|id| !id.is_empty())
    {
        Some(physical_id) => TemplateSource::Stack(physical_id.clone()),
        None => TemplateSource::Unaddressed,
    }
}

/// The CloudFormation stack a provisioned product's launch stands up, as AWS names it:
/// `SC-<account id>-<provisioned product id>`. One provisioned product is one stack, so
/// the plan of a change to it is a change set on that stack.
pub fn provisioned_product_stack_name(provisioned_product_id: &str) -> String {
    format!(
        "SC-{}-{provisioned_product_id}",
        ::emulator_server::EMULATOR_ACCOUNT_ID
    )
}

/// The change a plan plans, as the change set CloudFormation is asked to compute it.
#[derive(Debug, Clone)]
pub struct PlannedChange {
    /// The change set's name, which is the plan's id: it addresses the plan in both
    /// services.
    pub change_set_name: String,
    /// The stack the change set is computed against
    /// ([`provisioned_product_stack_name`]).
    pub stack_name: String,
    /// Where the product version's template is.
    pub template: TemplateSource,
    /// The parameters the request asked the launch to be planned with.
    pub parameters: Vec<UpdateProvisioningParameter>,
    /// The plan's tags, which the change set carries onto the stack it would deploy.
    pub tags: BTreeMap<String, String>,
    /// "Passed to CloudFormation. The SNS topic ARNs to which to publish stack-related
    /// events."
    pub notification_arns: Vec<String>,
}

/// What the change set behind a plan says about that plan right now: the plan's status,
/// the message that status carries, and the resource changes executing it would make.
#[derive(Debug, Clone)]
pub struct PlanReport {
    /// A `ProvisionedProductPlanStatus` value.
    pub status: String,
    /// The reason a failed plan failed, as CloudFormation gave it; `None` for a plan
    /// nothing has reported on.
    pub status_message: Option<String>,
    /// "Information about the resource changes that will occur when the plan is
    /// executed."
    pub resource_changes: Vec<ResourceChange>,
}

/// Ask CloudFormation to compute the change set `planned` describes, and answer its id —
/// or the reason CloudFormation could not plan the change at all, which is what a plan
/// that failed to be created reports as its status message.
///
/// The change set is created as a CREATE one when the stack it plans does not exist yet,
/// and as an UPDATE one when it does: "you can't use the UPDATE type to create a change
/// set for a new stack or the CREATE type to create a change set for an existing stack".
/// A stack a previous CREATE change set brought into existence carries no template yet —
/// it is in `REVIEW_IN_PROGRESS` until that change set is executed — so there is still
/// nothing to update, and the plan of it is a CREATE change set too.
pub async fn plan_change_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    planned: &PlannedChange,
) -> Result<String, String> {
    let cloudformation = in_process_cloudformation(ctx, region);
    let deployed = cloudformation
        .describe_stacks()
        .stack_name(&planned.stack_name)
        .send()
        .await
        .ok()
        .and_then(|described| {
            described
                .stacks
                .unwrap_or_default()
                .into_iter()
                .next()
                .and_then(|stack| stack.stack_status)
        })
        .is_some_and(|status| status.as_str() != STACK_STATUS_REVIEW_IN_PROGRESS);
    let change_set_type = if deployed {
        ::aws_sdk_cloudformation::types::ChangeSetType::Update
    } else {
        ::aws_sdk_cloudformation::types::ChangeSetType::Create
    };

    let mut request = cloudformation
        .create_change_set()
        .change_set_name(&planned.change_set_name)
        .stack_name(&planned.stack_name)
        .change_set_type(change_set_type)
        // Service Catalog submits the product's own template on the administrator's
        // behalf — the catalogue vetted the product, not each launch — so the
        // capabilities a vetted template may declare are acknowledged here rather than
        // asked of the end user, who never sees the template.
        .capabilities(::aws_sdk_cloudformation::types::Capability::CapabilityIam)
        .capabilities(::aws_sdk_cloudformation::types::Capability::CapabilityNamedIam)
        .capabilities(::aws_sdk_cloudformation::types::Capability::CapabilityAutoExpand);
    for arn in &planned.notification_arns {
        request = request.notification_arns(arn);
    }
    for (key, value) in &planned.tags {
        request = request.tags(
            ::aws_sdk_cloudformation::types::Tag::builder()
                .key(key)
                .value(value)
                .build(),
        );
    }
    for parameter in &planned.parameters {
        let mut submitted = ::aws_sdk_cloudformation::types::Parameter::builder()
            .set_parameter_key(parameter.key.clone())
            // "If set to true, `Value` is ignored and the previous parameter value is
            // kept", which is the same member CloudFormation's own parameter carries.
            .use_previous_value(parameter.use_previous_value);
        if !parameter.use_previous_value {
            submitted = submitted.set_parameter_value(parameter.value.clone());
        }
        request = request.parameters(submitted.build());
    }
    let request = match &planned.template {
        TemplateSource::Url(url) => request.template_url(url),
        TemplateSource::Stack(physical_id) => {
            // The version names the stack that CONTAINS the template, and a change set
            // is computed from a body or a URL: the body is the one that stack carries,
            // which CloudFormation is also what answers.
            let body = cloudformation
                .get_template()
                .stack_name(physical_id)
                .send()
                .await
                .map_err(error_message)?
                .template_body
                .ok_or_else(|| {
                    format!("The stack {physical_id} carries no template to plan from.")
                })?;
            request.template_body(body)
        }
        TemplateSource::Unaddressed => {
            return Err(
                "The provisioning artifact names no CloudFormation template to plan from."
                    .to_string(),
            )
        }
    };

    request
        .send()
        .await
        .map_err(error_message)?
        .id
        .ok_or_else(|| "CloudFormation created no change set to plan from.".to_string())
}

/// What the change set `change_set` names says about the plan it backs.
///
/// A plan whose change set has not been recorded yet is still being created; one
/// CloudFormation refused to plan carries the reason it gave; otherwise the change set
/// itself is asked, and its status, its reason and its resource changes ARE the plan's.
/// A change set that cannot be read says nothing about a plan that can still be
/// executed, so that plan reports the failure rather than an empty change list.
pub async fn plan_report(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    change_set: Option<&super::PlanChangeSet>,
) -> PlanReport {
    let id = match change_set {
        None => {
            return PlanReport {
                status: super::PLAN_STATUS_CREATE_IN_PROGRESS.to_string(),
                status_message: None,
                resource_changes: Vec::new(),
            }
        }
        Some(super::PlanChangeSet::Refused(reason)) => {
            return PlanReport {
                status: super::PLAN_STATUS_CREATE_FAILED.to_string(),
                status_message: Some(reason.clone()),
                resource_changes: Vec::new(),
            }
        }
        Some(super::PlanChangeSet::Created(id)) => id,
    };
    let described = in_process_cloudformation(ctx, region)
        .describe_change_set()
        .change_set_name(id)
        .send()
        .await;
    let described = match described {
        Ok(described) => described,
        Err(error) => {
            ::tracing::debug!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the change set behind this plan could not be read"
            );
            return PlanReport {
                status: super::PLAN_STATUS_CREATE_FAILED.to_string(),
                status_message: Some(error_message(error)),
                resource_changes: Vec::new(),
            };
        }
    };
    PlanReport {
        status: plan_status(described.status.as_ref().map(|status| status.as_str())).to_string(),
        status_message: described.status_reason.clone(),
        resource_changes: described
            .changes
            .unwrap_or_default()
            .iter()
            .filter_map(|change| change.resource_change.as_ref())
            .map(resource_change)
            .collect(),
    }
}

/// The `ProvisionedProductPlanStatus` a change set in `change_set_status` puts its plan
/// in: the creation is in flight while the change set is being computed
/// ("CloudFormation starts creating the change set" after the call that asks for it), it
/// succeeded once the change set is complete, and any other status is a creation that
/// did not succeed.
fn plan_status(change_set_status: Option<&str>) -> &'static str {
    match change_set_status {
        Some(CHANGE_SET_CREATE_PENDING) | Some(CHANGE_SET_CREATE_IN_PROGRESS) => {
            super::PLAN_STATUS_CREATE_IN_PROGRESS
        }
        Some(CHANGE_SET_CREATE_COMPLETE) => super::PLAN_STATUS_CREATE_SUCCESS,
        _ => super::PLAN_STATUS_CREATE_FAILED,
    }
}

/// `ChangeSetStatus`'s value for a change set CloudFormation has not started computing.
const CHANGE_SET_CREATE_PENDING: &str = "CREATE_PENDING";

/// `ChangeSetStatus`'s value for one it is computing.
const CHANGE_SET_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// `ChangeSetStatus`'s value for one it has computed.
const CHANGE_SET_CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// `StackStatus`'s value for a stack a CREATE change set brought into existence and
/// nothing has executed: it carries no template and no resources yet.
const STACK_STATUS_REVIEW_IN_PROGRESS: &str = "REVIEW_IN_PROGRESS";

/// One change CloudFormation computed, as Service Catalog reports it.
///
/// A plan IS the change set, so the change is RELAYED: both services describe a
/// resource change with the same members, and each value — the action, the replacement,
/// the scope, the evaluation and the attribute each detail targets — is the one
/// CloudFormation computed, in CloudFormation's own spelling (`Add`, `Modify`,
/// `Remove`), which is what a caller comparing a plan against the change set behind it
/// reads. Nothing is translated or dropped here: a value this service invented would
/// not be the plan CloudFormation made.
fn resource_change(changed: &::aws_sdk_cloudformation::types::ResourceChange) -> ResourceChange {
    ResourceChange {
        action: changed
            .action
            .as_ref()
            .map(|action| action.as_str().to_string()),
        logical_resource_id: changed.logical_resource_id.clone(),
        physical_resource_id: changed.physical_resource_id.clone(),
        resource_type: changed.resource_type.clone(),
        replacement: changed
            .replacement
            .as_ref()
            .map(|replacement| replacement.as_str().to_string()),
        scope: Some(
            changed
                .scope
                .iter()
                .flatten()
                .map(|attribute| attribute.as_str().to_string())
                .collect(),
        ),
        details: Some(
            changed
                .details
                .iter()
                .flatten()
                .map(resource_change_detail)
                .collect(),
        ),
    }
}

/// One attribute change of one resource, relayed the same way.
fn resource_change_detail(
    detail: &::aws_sdk_cloudformation::types::ResourceChangeDetail,
) -> ResourceChangeDetail {
    ResourceChangeDetail {
        target: detail
            .target
            .as_ref()
            .map(|target| ResourceTargetDefinition {
                attribute: target
                    .attribute
                    .as_ref()
                    .map(|attribute| attribute.as_str().to_string()),
                name: target.name.clone(),
                requires_recreation: target
                    .requires_recreation
                    .as_ref()
                    .map(|recreation| recreation.as_str().to_string()),
            }),
        evaluation: detail
            .evaluation
            .as_ref()
            .map(|evaluation| evaluation.as_str().to_string()),
        causing_entity: detail.causing_entity.clone(),
    }
}

/// What a failed CloudFormation call said, as the plan that could not be computed
/// reports it: the reason belongs to the service that refused, so it is passed on rather
/// than reworded.
fn error_message<E: std::error::Error>(error: E) -> String {
    ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
}

/// A CloudFormation client whose transport is the framework's IN-PROCESS callback into
/// this same emulator — the one way this crate reaches another emulated service.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it dispatches to the service mounted beside this one INSIDE this process. No socket
/// is opened, no host name is resolved, nothing outside this process is reachable
/// through it, and no request can steer it somewhere else — so a value a request
/// carries (a template URL, a stack name, a Region) is addressing for an in-process
/// dispatch and never a destination this process connects to.
///
/// The endpoint URL exists because the SDK requires one; its host is never resolved.
/// The only thing a request contributes to it is a well-formed Region token
/// ([`addressable_region`]), and the credentials are the emulator's own — the caller's
/// `Authorization` header is never forwarded. The transport is in-process, so there is
/// nothing to retry, time out or watch for a stalled stream, and each of those would
/// need a sleep implementation this client is not given.
pub fn in_process_cloudformation(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_cloudformation::Client {
    let region = addressable_region(region);
    let config = ::aws_sdk_cloudformation::Config::builder()
        .behavior_version(::aws_sdk_cloudformation::config::BehaviorVersion::latest())
        .region(::aws_sdk_cloudformation::config::Region::new(region.clone()))
        .endpoint_url(format!("https://cloudformation.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_cloudformation::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudformation::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudformation::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudformation::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudformation::Client::from_conf(config)
}

/// `region` if it is a well-formed Region token — the lowercase letters, digits and
/// hyphens an AWS Region name is made of, within the longest one AWS publishes — and
/// the emulator's default Region for anything else.
///
/// The Region a request was signed for is the only part of this crate's in-process
/// addressing a caller contributes, so it is narrowed to that shape before it is used:
/// an in-process dispatch cannot be redirected by it, and nothing else it could be
/// mistaken for survives.
fn addressable_region(region: &str) -> String {
    let well_formed = !region.is_empty()
        && region.len() <= MAX_REGION_LENGTH
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    match well_formed {
        true => region.to_string(),
        false => super::DEFAULT_REGION.to_string(),
    }
}

/// The length a Region token is kept within: AWS's longest published Region name
/// (`ap-southeast-7`, `il-central-1`, `us-gov-east-1` and their kind) is far shorter,
/// and nothing longer addresses an emulated service.
const MAX_REGION_LENGTH: usize = 32;
