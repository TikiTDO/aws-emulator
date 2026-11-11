// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the application services: IAM users' access keys, AWS
//! CodePipeline, Amazon ECS, Amazon SQS, Amazon SNS, Amazon SageMaker and AWS Fault Injection
//! Service. One file of [`super`]'s per-type arms, dispatched to by its `Provisioner`.

use super::*;
// The arms of the database and network types this group holds, split out so each file stays
// readable; re-exported, so this module is the one path to them.
mod databases_and_networks;
pub(crate) use databases_and_networks::*;

/// The path IAM gives a user created without one ("if it is not included, it defaults to a
/// slash", `CreateUser$Path`), which is what a user goes back to when a deployment stops
/// declaring `Path`.
///
/// Read from the IAM crate, which owns that default and applies it to every entity it
/// creates, so the two cannot drift apart.
pub(super) const DEFAULT_IAM_PATH: &str =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::DEFAULT_PATH;

/// Bring the tags of the user a resource record names to the ones this deployment gives it,
/// through IAM's own `TagUser` and `UntagUser`.
///
/// The set is the declaration's own `Tags` beside the ones CloudFormation propagates
/// ([`tags_of`]) — the same set [`create_iam_user`] applied — read back from `GetUser`,
/// which reports a user's tags on the user itself. Where the two sets already agree nothing
/// is called at all, which is what makes this safe to ask on every deployment. The same
/// reconciliation [`retag_role`] makes for a role's tags.
pub(super) async fn retag_iam_user(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let desired = tags_of(declared, propagated);
    let client = iam_client(ctx, region);
    let user_name = &resource.physical_resource_id;
    let held: Vec<(String, String)> = match client.get_user().user_name(user_name).send().await {
        Ok(answer) => answer
            .user()
            .map(|user| {
                user.tags()
                    .iter()
                    .map(|tag| (tag.key().to_string(), tag.value().to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(());
            }
            return Err(refusal);
        }
    };
    let mut added = Vec::new();
    for (key, value) in desired.iter().filter(|(key, value)| {
        held.iter()
            .find(|(held_key, _)| held_key == key)
            .is_none_or(|(_, held_value)| held_value != value)
    }) {
        added.push(
            ::aws_sdk_iam::types::Tag::builder()
                .key(key)
                .value(value)
                .build()
                .map_err(|error| Refusal {
                    code: UNIDENTIFIED_FAILURE.to_string(),
                    message: format!("the tag {key} cannot be sent to IAM: {error}"),
                })?,
        );
    }
    let removed: Vec<String> = held
        .iter()
        .filter(|(key, _)| !desired.iter().any(|(wanted, _)| wanted == key))
        .map(|(key, _)| key.clone())
        .collect();
    if !added.is_empty() {
        client
            .tag_user()
            .user_name(user_name)
            .set_tags(Some(added))
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    if !removed.is_empty() {
        client
            .untag_user()
            .user_name(user_name)
            .set_tag_keys(Some(removed))
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    Ok(())
}

/// Delete the user a stack created. "You must delete the items attached to the user
/// manually, or the deletion fails" (`DeleteUser`) — the console password, the inline
/// policies, the attached managed policies and the group memberships the template declared
/// each go first, in the order that operation's own documentation lists them. A user, a
/// policy or a membership IAM no longer has is already in the state the delete asks for.
pub(super) async fn delete_iam_user(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let client = iam_client(ctx, region);
    let user_name = &resource.physical_resource_id;
    if declared_login_profile(&resource.properties).is_some() {
        if let Err(error) = client
            .delete_login_profile()
            .user_name(user_name)
            .send()
            .await
        {
            tolerated_absence(&error)?;
        }
    }
    for policy in inline_policies(&resource.properties) {
        let Some(name) = policy.get("PolicyName").and_then(scalar_text) else {
            continue;
        };
        if let Err(error) = client
            .delete_user_policy()
            .user_name(user_name)
            .policy_name(name)
            .send()
            .await
        {
            tolerated_absence(&error)?;
        }
    }
    for policy_arn in managed_policy_arns(&resource.properties) {
        if let Err(error) = client
            .detach_user_policy()
            .user_name(user_name)
            .policy_arn(policy_arn)
            .send()
            .await
        {
            tolerated_absence(&error)?;
        }
    }
    for group in declared_groups(&resource.properties) {
        if let Err(error) = client
            .remove_user_from_group()
            .user_name(user_name)
            .group_name(group)
            .send()
            .await
        {
            tolerated_absence(&error)?;
        }
    }
    match client.delete_user().user_name(user_name).send().await {
        Ok(_) => Ok(()),
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(());
            }
            Err(refusal)
        }
    }
}

/// The refusal of a delete step that the resource is already PAST: IAM no longer has the
/// entity, the policy or the membership the step would have removed, which is the state the
/// step asks for. Judged on IAM's own not-found CODE, never on a message, exactly as
/// [`Provisioner::delete`] judges a resource its service no longer holds.
pub(super) fn tolerated_absence<E>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> Result<(), Refusal>
where
    ::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::fmt::Display,
{
    let refusal = iam_refusal_of(error);
    if refusal.code == NO_SUCH_ENTITY {
        return Ok(());
    }
    Err(refusal)
}

/// The user's configuration as IAM reports it, for drift detection.
///
/// A user IAM no longer holds (`NoSuchEntity`) was deleted. The one member compared is the
/// one the resource type and `GetUser` spell identically and the create sends — `Path` — so
/// a user moved to another path outside the stack is MODIFIED.
pub(super) async fn describe_iam_user(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answer = match iam_client(ctx, region)
        .get_user()
        .user_name(physical_resource_id)
        .send()
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(None);
            }
            return Err(refusal);
        }
    };
    let mut actual = Map::new();
    if let Some(user) = answer.user() {
        actual.insert("Path".to_string(), Value::String(user.path().to_string()));
    }
    Ok(Some(Value::Object(actual)))
}

// ---------------------------------------------------------------------------
// IAM: AWS::IAM::AccessKey
// ---------------------------------------------------------------------------

/// The property an `AWS::IAM::AccessKey` declaration names the user its key belongs to in:
/// "the name of the IAM user that the new key will belong to", which is the `UserName`
/// `CreateAccessKey` takes.
pub(super) const ACCESS_KEY_USER_NAME: &str = "UserName";

/// The property it declares the key's status in: "the status of the access key. `Active`
/// means that the key is valid for API calls, while `Inactive` means it is not" — the values
/// IAM's own `statusType` spells.
pub(super) const ACCESS_KEY_STATUS: &str = "Status";

/// The status every minted key carries: "the default status for new keys is Active"
/// (`CreateAccessKey`), as `statusType` spells it.
pub(super) const ACCESS_KEY_ACTIVE: &str = "Active";

/// The one property of an access key that already exists IAM can change: its status, which
/// `UpdateAccessKey` moves. The user the key belongs to and the `Serial` a template rotates
/// it with are create-only ([`super::template::replacement_properties`]), so a declaration
/// that states either differently REPLACES the resource and never reaches
/// [`update_iam_access_key`].
pub(super) const ACCESS_KEY_IN_PLACE_PROPERTIES: &[&str] = &[ACCESS_KEY_STATUS];

/// Create the access key an `AWS::IAM::AccessKey` declares for the user its declaration
/// names, and report the identity IAM minted: the access key id this type's `Ref` returns
/// and the secret its `SecretAccessKey` attribute carries.
///
/// Both are IAM's to generate — `CreateAccessKey` "creates a new Amazon Web Services secret
/// access key and corresponding Amazon Web Services access key ID for the specified user",
/// and the secret "is returned only in response to CreateAccessKey" — so this one answer is
/// the only place either value can come from, and nothing here computes one. A user IAM does
/// not hold, and a user already at its key quota, are that service's own refusals, which
/// fail the resource like any other: a stack never publishes a credential nothing minted.
///
/// Every key is minted `Active`, so a declaration that asks for another status is applied
/// with the one IAM call that moves it — the same call a client would make next. A status
/// that call refuses leaves no resource, so the key it just minted is withdrawn before the
/// refusal is reported rather than left on the user.
pub(super) async fn create_iam_access_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let client = iam_client(ctx, region);
    let user_name = required_property(resource, ACCESS_KEY_USER_NAME)?;
    let answer = client
        .create_access_key()
        .user_name(&user_name)
        .send()
        .await
        .map_err(|error| iam_refusal_of(&error))?;
    let Some(minted) = answer.access_key() else {
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "IAM answered CreateAccessKey without an AccessKey".to_string(),
        });
    };
    let access_key_id = minted.access_key_id().to_string();
    let mut attributes = BTreeMap::new();
    attributes.insert(
        "SecretAccessKey".to_string(),
        minted.secret_access_key().to_string(),
    );
    if let Some(status) =
        property(resource, ACCESS_KEY_STATUS).filter(|status| status != ACCESS_KEY_ACTIVE)
    {
        if let Err(refusal) = client
            .update_access_key()
            .user_name(&user_name)
            .access_key_id(&access_key_id)
            .status(::aws_sdk_iam::types::StatusType::from(status.as_str()))
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))
        {
            // The key is not the one the declaration asked for and the resource is not
            // created, so the credential this arm just minted is WITHDRAWN before the
            // refusal is reported: the stack publishes no id for it, so neither its own
            // rollback nor its deletion could delete it afterwards — and a user that still
            // holds an access key is one IAM refuses to delete.
            client
                .delete_access_key()
                .user_name(&user_name)
                .access_key_id(&access_key_id)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
            return Err(refusal);
        }
    }
    Ok(Created {
        physical_resource_id: Some(access_key_id),
        attributes,
    })
}

/// Apply a CHANGED `AWS::IAM::AccessKey` declaration to the key the stack already has,
/// through `UpdateAccessKey` — "changes the status of the specified access key from Active
/// to Inactive, or vice versa", which is what deactivating a deployed credential is.
///
/// The key therefore keeps its identity: the same id and the same secret, so an output that
/// published either still resolves to the credential the stack deployed. A status the
/// deployment no longer declares goes back to the one a newly minted key carries, so a
/// dropped property is reset rather than silently kept.
///
/// `Ok(None)` is a change no IAM call makes to a key that exists — a property outside
/// [`ACCESS_KEY_IN_PLACE_PROPERTIES`] — which the deployment then reports as not applied.
pub(super) async fn update_iam_access_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if changed_properties(&resource.properties, declared)
        .iter()
        .any(|property| !ACCESS_KEY_IN_PLACE_PROPERTIES.contains(&property.as_str()))
    {
        return Ok(None);
    }
    let status = declared
        .get(ACCESS_KEY_STATUS)
        .and_then(scalar_text)
        .unwrap_or_else(|| ACCESS_KEY_ACTIVE.to_string());
    iam_client(ctx, region)
        .update_access_key()
        .user_name(required_property(resource, ACCESS_KEY_USER_NAME)?)
        .access_key_id(&resource.physical_resource_id)
        .status(::aws_sdk_iam::types::StatusType::from(status.as_str()))
        .send()
        .await
        .map_err(|error| iam_refusal_of(&error))?;
    // The id and the secret are the ones the create reported and are unchanged by this
    // call, so the attributes in force are the ones the resource already carries.
    Ok(Some(Created::attributes(BTreeMap::new())))
}

/// Delete the access key a stack created, so the credential it published stops signing
/// requests.
///
/// `DeleteAccessKey` addresses a key by the PAIR IAM keys one by — the user that holds it
/// and the id IAM minted — so the user half comes off the resource's own declaration. A key
/// IAM no longer has, and a user it no longer has, are both already in the state the delete
/// asks for ([`tolerated_absence`]).
pub(super) async fn delete_iam_access_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    match iam_client(ctx, region)
        .delete_access_key()
        .user_name(required_property(resource, ACCESS_KEY_USER_NAME)?)
        .access_key_id(&resource.physical_resource_id)
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => tolerated_absence(&error),
    }
}

/// The access key's configuration as IAM reports it, for drift detection.
///
/// IAM publishes no read of ONE access key — "returns information about the access key IDs
/// associated with the specified IAM user" is the read it has — so the key is looked up in
/// its user's own list, which carries every key that user holds (IAM bounds the keys per
/// user, so one page is all of them). A key that list no longer carries, and a user IAM no
/// longer holds, were deleted. The members compared are the two the resource type and
/// `AccessKeyMetadata` spell identically and the create sends — `UserName` and `Status` — so
/// a key deactivated outside the stack is MODIFIED.
pub(super) async fn describe_iam_access_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let user_name = required_property(resource, ACCESS_KEY_USER_NAME)?;
    let answer = match iam_client(ctx, region)
        .list_access_keys()
        .user_name(&user_name)
        .send()
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(None);
            }
            return Err(refusal);
        }
    };
    let Some(held) = answer
        .access_key_metadata()
        .iter()
        .find(|metadata| metadata.access_key_id() == Some(resource.physical_resource_id.as_str()))
    else {
        return Ok(None);
    };
    let mut actual = Map::new();
    if let Some(holder) = held.user_name() {
        actual.insert(
            ACCESS_KEY_USER_NAME.to_string(),
            Value::String(holder.to_string()),
        );
    }
    if let Some(status) = held.status() {
        actual.insert(
            ACCESS_KEY_STATUS.to_string(),
            Value::String(status.as_str().to_string()),
        );
    }
    Ok(Some(Value::Object(actual)))
}

// ---------------------------------------------------------------------------
// CodePipeline: AWS::CodePipeline::Pipeline
// ---------------------------------------------------------------------------
/// CodePipeline's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: codepipeline`, service shape `CodePipeline_20150709`).
pub(super) const CODEPIPELINE_SIGNING_NAME: &str = "codepipeline";
pub(super) const CODEPIPELINE_TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The error code CodePipeline answers when a pipeline of that name already exists, which
/// is the resource this deployment wanted.
pub(super) const PIPELINE_NAME_IN_USE: &str = "PipelineNameInUseException";

/// The error code CodePipeline answers for a pipeline it does not hold.
pub(super) const PIPELINE_NOT_FOUND: &str = "PipelineNotFoundException";

/// Create the pipeline an `AWS::CodePipeline::Pipeline` declares.
///
/// This type's CloudFormation properties ARE `CreatePipeline`'s `PipelineDeclaration`,
/// member for member — `RoleArn`/`roleArn`, `ArtifactStore`/`artifactStore`,
/// `Stages`/`stages`, `ActionTypeId`/`actionTypeId` and the rest
/// (`smithy_query query="shape" shape="PipelineDeclaration"`) — differing only in the case
/// of each member's initial letter, so the evaluated property document is carried through
/// with that one transformation ([`camel_cased`]) rather than mapped member by member.
///
/// Three properties are not members of the declaration and are taken out of it: `Tags` is
/// a member of the `CreatePipeline` REQUEST beside the pipeline, and
/// `DisableInboundStageTransitions` and `RestartExecutionOnUpdate` are CloudFormation's
/// own (a held transition is `DisableStageTransition`'s business, and restart-on-update is
/// a deployment instruction) — so the request carries only what the model declares.
///
/// The pipeline carries the tags the declaration asks for plus the ones CloudFormation
/// gives the resource ([`tags_of`]), as the list CodePipeline's own `Tag` shape spells in
/// lower camel case.
pub(super) async fn create_pipeline(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut declaration = match camel_cased(&resource.properties, 0)? {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    // The one member CloudFormation itself decides.
    declaration.insert(
        "name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    // `Tags` is the request's own member, not the declaration's, so it comes out of the
    // document the pipeline is.
    declaration.remove("tags");
    declaration.remove("disableInboundStageTransitions");
    declaration.remove("restartExecutionOnUpdate");
    // `ArtifactStores` is a LIST of `{Region, ArtifactStore}` in a template and
    // `ArtifactStoreMap`, keyed by Region, in the model: the same information either way.
    if let Some(stores) = declaration.remove("artifactStores") {
        let rewritten = match stores.as_array() {
            Some(entries) => {
                let mut by_region = Map::new();
                for entry in entries {
                    let (Some(entry_region), Some(store)) = (
                        entry.get("region").and_then(Value::as_str),
                        entry.get("artifactStore"),
                    ) else {
                        continue;
                    };
                    by_region.insert(entry_region.to_string(), store.clone());
                }
                Value::Object(by_region)
            }
            None => stores,
        };
        declaration.insert("artifactStores".to_string(), rewritten);
    }
    let mut request = Map::new();
    request.insert("pipeline".to_string(), Value::Object(declaration));
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    match call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.CreatePipeline"),
        &Value::Object(request),
    )
    .await
    {
        Ok(_) => Ok(Created::default()),
        Err(refusal) if refusal.code == PIPELINE_NAME_IN_USE => Ok(Created::default()),
        Err(refusal) => Err(refusal),
    }
}

/// Delete the pipeline a stack created. One CodePipeline no longer has is already in the
/// state the delete asks for.
pub(super) async fn delete_pipeline(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.DeletePipeline"),
        &serde_json::json!({ "name": physical_resource_id }),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == PIPELINE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The pipeline's declaration as CodePipeline reports it, for drift detection. One it no
/// longer holds (`PipelineNotFoundException`) was deleted.
pub(super) async fn describe_pipeline(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.GetPipeline"),
        &serde_json::json!({ "name": physical_resource_id }),
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_member(&answer, "pipeline"))),
        Err(refusal) if refusal.code == PIPELINE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `ActionOwner` of every action type `CreateCustomActionType` registers ("used for
/// custom actions only"), which is also the owner a poll for one of their jobs names.
pub(super) const CODEPIPELINE_OWNER_CUSTOM: &str = "Custom";

/// The three properties that IDENTIFY an `AWS::CodePipeline::CustomActionType`, which are
/// the three members `DeleteCustomActionType` addresses a registration by.
pub(super) const CODEPIPELINE_ACTION_TYPE_IDENTITY: &[&str] = &["Category", "Provider", "Version"];

/// Register the custom action type an `AWS::CodePipeline::CustomActionType` declares — so
/// a job worker that polls for the action type the stack declared is SERVED, and the
/// pipeline the same stack declares can have its custom action claimed, rather than
/// CodePipeline refusing an action type its own completed stack was supposed to register.
///
/// This type's CloudFormation properties ARE `CreateCustomActionType`'s own input, member
/// for member — `Category`/`category`, `Provider`/`provider`, `Version`/`version`,
/// `ConfigurationProperties`/`configurationProperties`,
/// `InputArtifactDetails`/`inputArtifactDetails`,
/// `OutputArtifactDetails`/`outputArtifactDetails`, `Settings`/`settings` and the members
/// nested inside them (`smithy_query query="operation" operation="CreateCustomActionType"`)
/// — differing only in the case of each member's initial letter, so the evaluated property
/// document is carried through with that one transformation ([`camel_cased`]) rather than
/// mapped member by member.
///
/// `Tags` is taken out of the document and sent as the request's own member, carrying the
/// tags the declaration asks for plus the ones CloudFormation gives the resource
/// ([`tags_of`]), as the list CodePipeline's own `Tag` shape spells in lower camel case.
pub(super) async fn create_custom_action_type(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = match camel_cased(&resource.properties, 0)? {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    // `Tags` is the request's own member rather than part of the action type.
    request.remove("tags");
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.CreateCustomActionType"),
        &Value::Object(request),
    )
    .await?;
    Ok(Created::default())
}

/// Remove the registration a stack created, addressed by the triple that identifies it.
///
/// `DeleteCustomActionType` models no does-not-exist error: an identity CodePipeline holds
/// no registration of is already in the state the delete asks for. A declaration that
/// states none of the triple names no registration at all, so there is nothing to remove.
pub(super) async fn delete_custom_action_type(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let Some(identity) = custom_action_type_identity(resource) else {
        return Ok(());
    };
    call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.DeleteCustomActionType"),
        &Value::Object(identity),
    )
    .await?;
    Ok(())
}

/// The registration as CodePipeline reports it, for drift detection: the identifying
/// triple of the action type the account holds under this declaration's identity.
///
/// `ListActionTypes` reports the action types the account holds rather than refusing a
/// read of one it does not, so an answer that lists no action type of this identity is
/// that service saying it no longer holds the resource — the `DELETED` drift status.
pub(super) async fn describe_custom_action_type(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let Some(identity) = custom_action_type_identity(resource) else {
        return Ok(None);
    };
    let answered = call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.ListActionTypes"),
        &serde_json::json!({ "actionOwnerFilter": CODEPIPELINE_OWNER_CUSTOM }),
    )
    .await?;
    let held = answered
        .get("actionTypes")
        .and_then(Value::as_array)
        .is_some_and(|action_types| {
            action_types.iter().any(|action_type| {
                CODEPIPELINE_ACTION_TYPE_IDENTITY.iter().all(|property| {
                    action_type
                        .get("id")
                        .and_then(|id| id.get(lower_initial(property)))
                        == identity.get(&lower_initial(property))
                })
            })
        });
    if !held {
        return Ok(None);
    }
    let reported: Map<String, Value> = identity
        .into_iter()
        .filter_map(|(member, value)| {
            let property = CODEPIPELINE_ACTION_TYPE_IDENTITY
                .iter()
                .find(|property| lower_initial(property) == member)?;
            Some(((*property).to_string(), value))
        })
        .collect();
    Ok(Some(Value::Object(reported)))
}

/// The members `DeleteCustomActionType` takes for the registration a resource record
/// describes, read off the declaration that made it: the three properties that identify
/// an action type, under that call's own member names.
///
/// `None` when the declaration states none of them as a scalar — the record names no
/// registration, so no call can address one.
pub(super) fn custom_action_type_identity(resource: &StoredResource) -> Option<Map<String, Value>> {
    let mut identity = Map::new();
    for property in CODEPIPELINE_ACTION_TYPE_IDENTITY {
        let value = resource.properties.get(*property).and_then(scalar_text)?;
        identity.insert(lower_initial(property), Value::String(value));
    }
    Some(identity)
}

// ---------------------------------------------------------------------------
// Reading an evaluated property document
// ---------------------------------------------------------------------------

/// The deepest a re-casing walk descends into an evaluated property document.
///
/// The same rule, for the same reason, as [`MAX_CONFIGURATION_NESTING`]: the properties
/// come from a caller-supplied template body, so their nesting is the REQUESTER's choice
/// and a recursive walk over them has to be bounded by a constant rather than by the
/// input — exhausting the native stack ABORTS the process instead of unwinding, which
/// would take the emulator down for every caller rather than failing one stack. A document
/// nested deeper than this is REFUSED, never quietly truncated: a resource created from
/// part of the document its template declared is not the resource the template described.
///
/// Set to the template reader's own [`super::template::MAX_TEMPLATE_DEPTH`], the deepest
/// body that reader admits at all, so this bound refuses nothing a template could carry
/// and still holds when a property document reaches a walk from somewhere else.
pub(super) const MAX_PROPERTY_NESTING: usize = super::template::MAX_TEMPLATE_DEPTH;

/// The same document with every object KEY's initial letter lower-cased, which is the one
/// difference between a CloudFormation resource type's property names and the API members
/// they are (`RoleArn` -> `roleArn`).
///
/// An `ActionDeclaration`'s `configuration` is the exception: `ActionConfigurationMap`'s
/// keys are the action PROVIDER's own, case-sensitive settings (`S3Bucket`,
/// `PollForSourceChanges`), not modelled member names, so the entries of that one map are
/// carried through verbatim.
///
/// `depth` is how many levels are already above this one, so the outermost call passes
/// `0`; a document deeper than [`MAX_PROPERTY_NESTING`] is refused rather than walked.
pub(super) fn camel_cased(value: &Value, depth: usize) -> Result<Value, Refusal> {
    /// The one map whose KEYS are data rather than member names.
    const CONFIGURATION: &str = "configuration";

    if depth >= MAX_PROPERTY_NESTING {
        return Err(too_deeply_nested());
    }
    match value {
        Value::Object(members) => {
            let mut converted = Map::new();
            for (key, member) in members {
                let name = lower_initial(key);
                let member = if name == CONFIGURATION {
                    member.clone()
                } else {
                    camel_cased(member, depth + 1)?
                };
                converted.insert(name, member);
            }
            Ok(Value::Object(converted))
        }
        Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                converted.push(camel_cased(item, depth + 1)?);
            }
            Ok(Value::Array(converted))
        }
        scalar => Ok(scalar.clone()),
    }
}

/// The refusal a property document nested past [`MAX_PROPERTY_NESTING`] answers: the
/// resource is not created, which is what [`configurations`] answers for the same case.
pub(super) fn too_deeply_nested() -> Refusal {
    Refusal {
        code: "ValidationException".to_string(),
        message: format!(
            "Resource properties are nested deeper than the {MAX_PROPERTY_NESTING} levels \
             this emulator carries, so the resource was not created"
        ),
    }
}

/// One name with its FIRST character lower-cased and nothing else touched, so an
/// initialism inside a member name keeps its case (`RoleARN` -> `roleARN`).
pub(super) fn lower_initial(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// One member of an answer document, or an empty object where the answer carried none: a
/// read that succeeded reported the resource, and a member it did not carry is no
/// configuration to compare rather than a missing resource.
pub(super) fn reported_member(answer: &Value, member: &str) -> Value {
    answer
        .get(member)
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()))
}

/// The FIRST entry of a list an answer carries, or `None` for an answer that lists none —
/// which is how a describe-many call reports a resource the service does not hold.
pub(super) fn first_listed(answer: &Value, member: &str) -> Option<Value> {
    answer
        .get(member)
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .cloned()
}

/// One JSON scalar as the text an API member takes, or `None` for a composite.
///
/// The one spelling rule, held in [`super::scalar_text`] so the create side and the drift
/// comparison that reads the resource back cannot disagree about it.
pub(super) fn scalar_text(value: &Value) -> Option<String> {
    super::scalar_text(value)
}

/// A policy-document property as the JSON text the IAM member takes.
///
/// CloudFormation accepts a policy document as an inline object (JSON or YAML) or as a
/// string, and IAM takes it as text, so an object is rendered and a string passes through.
pub(super) fn document_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) | Value::Array(_) => serde_json::to_string(value).ok(),
        _ => None,
    }
}

/// Render the named members of a forwarded request as the TEXT of the document each one
/// carries, in place.
///
/// An arm that forwards the evaluated property document as the request body sends each
/// property under the type its API member is modelled with, and a member modelled a STRING
/// that carries a JSON document (a policy, a pattern, a dashboard body) is one a template
/// may spell either as the document itself or as the text of one. So a member named here is
/// rendered with the one JSON-to-text rule this module has ([`document_text`]): text stays
/// itself, and a document becomes its own compact JSON. A member the declaration does not
/// carry, or one carrying a value that is no document at all, is left exactly as declared —
/// the owning service's own validation is what judges it.
pub(super) fn render_document_members(request: &mut Map<String, Value>, members: &[&str]) {
    for member in members {
        let Some(rendered) = request.get(*member).and_then(document_text) else {
            continue;
        };
        request.insert((*member).to_string(), Value::String(rendered));
    }
}

/// A property that carries a whole number, in either spelling a template gives one: JSON's
/// number, or the string a quoted YAML scalar leaves behind.
pub(super) fn whole_number(value: Option<&Value>) -> Option<i32> {
    let value = value?;
    let whole = match value {
        Value::Number(number) => number.as_i64()?,
        Value::String(text) => text.parse().ok()?,
        _ => return None,
    };
    i32::try_from(whole).ok()
}

// ---------------------------------------------------------------------------
// The SDK clients an arm calls its owning service through
// ---------------------------------------------------------------------------

/// The error code this module reports for a declaration that is missing a property its
/// resource type requires, which is the code CloudFormation's own template validation
/// answers with.
pub(super) const MISSING_PROPERTY_CODE: &str = "ValidationError";

/// The error code that stands in for one an answer did not carry: the call did not
/// succeed, and nothing on the other end identified the problem.
pub(super) const UNIDENTIFIED_FAILURE: &str = "InternalFailure";

/// The error code this module reports for a declared property whose VALUE the resource type
/// cannot take — text where an instant is called for, say. CloudFormation answers a property
/// fault with its template-validation code whether the property is absent or malformed, so
/// this carries [`MISSING_PROPERTY_CODE`]'s value under the name the malformed case reads by;
/// what tells the two apart is the message, which names the property and what is wrong with
/// it.
pub(super) const INVALID_PROPERTY_CODE: &str = MISSING_PROPERTY_CODE;

/// An SDK error as the [`Refusal`] the stack reports: the error code the owning service
/// NAMED and the message it worded, kept apart so the caller branches on the code.
///
/// An error carrying no code is still a refusal — the create did not succeed — reported
/// under [`UNIDENTIFIED_FAILURE`] with whatever the transport said.
pub(super) fn refusal_of<E>(error: &E) -> Refusal
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::fmt::Display,
{
    Refusal {
        code: error.code().unwrap_or(UNIDENTIFIED_FAILURE).to_string(),
        message: error
            .message()
            .map_or_else(|| error.to_string(), ToString::to_string),
    }
}

/// IAM's own signing name, which is how the diagnostic below looks up the name an SDK of
/// IAM reports it under (`super::provisioning::SDK_SERVICE_NAMES`).
pub(super) const IAM_SIGNING_NAME: &str = "iam";

/// Amazon S3's own signing name, for the same lookup.
pub(super) const S3_SIGNING_NAME: &str = "s3";

/// A refused call to another service as the [`Refusal`] the stack reports: [`refusal_of`]'s
/// code and message, with the diagnostic the SDK appends to the exception it raises for a
/// refused call ([`super::provisioning::sdk_message`]) — the service, the HTTP STATUS of the
/// answer that service gave, the id (or ids) of the call and the attempts made at it.
///
/// A resource provider does not word its own diagnosis of a refused call: what it reports
/// is the exception its SDK raised, which is the called service's message plus that
/// diagnostic, so the reason a stack carries for a resource an owning service refused is the
/// one a client of that service would read. The status comes from the answer itself — never
/// from the error code — so an answer that carried no response at all (the call was never
/// made) is reported exactly as the service worded it, with no diagnostic invented for it.
pub(super) fn sdk_refusal_of<E>(
    signing_name: &str,
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> Refusal
where
    ::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::fmt::Display,
{
    let refusal = refusal_of(error);
    match error.raw_response().map(|answer| answer.status().as_u16()) {
        Some(status) => Refusal {
            message: super::provisioning::sdk_message(
                signing_name,
                status,
                &refusal.code,
                &refusal.message,
            ),
            ..refusal
        },
        None => refusal,
    }
}

/// A refused IAM call as the [`Refusal`] the stack reports ([`sdk_refusal_of`]).
pub(super) fn iam_refusal_of<E>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> Refusal
where
    ::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::fmt::Display,
{
    sdk_refusal_of(IAM_SIGNING_NAME, error)
}

/// A refused Amazon S3 call as the [`Refusal`] the stack reports ([`sdk_refusal_of`]).
///
/// Every S3 call this module makes reports its refusals through this, so a bucket S3 would
/// not create, change or delete fails its resource with the exception an S3 client would
/// have raised — S3's own message plus its SDK's diagnostic.
pub(super) fn s3_refusal_of<E>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> Refusal
where
    ::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::fmt::Display,
{
    sdk_refusal_of(S3_SIGNING_NAME, error)
}

/// The endpoint URL an in-process call to another emulated service is addressed at.
///
/// The host is built from `service` and the FIXED [`super::DEFAULT_REGION`] literal, never
/// from template-derived text: the framework's callback routes such a call in process on
/// the service label, and the client carries the emulator's own credentials, so no value a
/// caller sends may reach the host position. The stack's own Region is what the call is
/// ABOUT, and is the client's signing scope; it is not part of the destination.
pub(super) fn in_process_endpoint(service: &str) -> String {
    format!(
        "https://{service}.{}.amazonaws.com",
        super::DEFAULT_REGION
    )
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process and signs with the emulator's own credentials, so no network
/// client is configured here. Path-style addressing is what the emulated S3 service routes
/// on (its operations are bound under a `{Bucket}` path label).
pub(crate) fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(in_process_endpoint("s3"))
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: nothing to retry, time out or watch for a stalled
        // stream, and each would need a sleep implementation this client is not given.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// An IAM client over the same in-process callback, configured the same way.
pub(super) fn iam_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_iam::Client {
    let config = ::aws_sdk_iam::Config::builder()
        .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
        .region(::aws_sdk_iam::config::Region::new(region.to_string()))
        .endpoint_url(in_process_endpoint("iam"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_iam::Client::from_conf(config)
}

// ---------------------------------------------------------------------------
// Amazon ECS: AWS::ECS::Cluster
// ---------------------------------------------------------------------------

/// Amazon ECS's signing name and `X-Amz-Target` prefix, from its model (`sigv4Name: ecs`,
/// service shape `AmazonEC2ContainerServiceV20141113`), which speaks `awsJson1_1`.
pub(super) const ECS_SIGNING_NAME: &str = "ecs";
pub(super) const ECS_TARGET_PREFIX: &str = "AmazonEC2ContainerServiceV20141113";

/// The error code Amazon ECS answers for a cluster it does not hold
/// (`ClusterNotFoundException`: "The specified cluster wasn't found").
pub(super) const CLUSTER_NOT_FOUND: &str = "ClusterNotFoundException";

/// The `AWS::ECS::Cluster` properties `CreateCluster` takes, as (property, member) pairs.
///
/// Source: AWS's CloudFormation resource reference for the type, member by member against
/// the `CreateClusterRequest` shape. The Amazon ECS model spells its members in LOWER
/// CAMEL case, so each value's own members are re-cased by [`lower_camel_members`], and
/// one property's documented name is not its member's at all — a cluster's
/// `ClusterSettings` sets `settings` ("The setting to use when creating a cluster").
/// A property these pairs do not name is one `CreateCluster` has no member for; the
/// cluster's `ClusterName` is set from the physical id instead, and its `tags` by
/// [`tags_of`], which carries the stack's propagated tags beside the declared ones.
pub(super) const ECS_CLUSTER_MEMBERS: &[(&str, &str)] = &[
    ("CapacityProviders", "capacityProviders"),
    ("ClusterSettings", "settings"),
    ("Configuration", "configuration"),
    (
        "DefaultCapacityProviderStrategy",
        "defaultCapacityProviderStrategy",
    ),
    ("ServiceConnectDefaults", "serviceConnectDefaults"),
];

/// Create the cluster an `AWS::ECS::Cluster` declares, so a client reading the stack's
/// outputs and then calling Amazon ECS against that cluster (`ListTasks`,
/// `DescribeClusters`) finds it there rather than a `ClusterNotFoundException`.
///
/// `Ref` of this type returns the cluster NAME, which is this resource's physical id, so
/// the cluster is created under exactly the name the stack reports for it; the ARN it
/// reports back is the resource's real identity and replaces the computed one.
///
/// The cluster carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as the list Amazon ECS's own `Tag` shape spells in lower
/// camel case — less the `aws:cloudformation:*` ones, which `CreateCluster` refuses to a
/// caller like the other reserved-namespace APIs ([`Provisioner::applicable_tags`]).
pub(super) async fn create_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    if let Value::Object(properties) = &resource.properties {
        for (property, member) in ECS_CLUSTER_MEMBERS {
            if let Some(value) = properties.get(*property) {
                request.insert((*member).to_string(), lower_camel_members(value, 0)?);
            }
        }
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    request.insert(
        "clusterName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let reported = call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.CreateCluster"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported
        .get("cluster")
        .and_then(|cluster| cluster.get("clusterArn"))
        .and_then(Value::as_str)
    {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    // The cluster was created under the name CloudFormation decided.
    Ok(Created::attributes(attributes))
}

/// Delete the cluster a stack created. A cluster Amazon ECS no longer holds
/// (`ClusterNotFoundException`) is already in the state the delete asks for; one that
/// still holds active services or tasks is NOT deleted, and that refusal is what the stack
/// reports.
pub(super) async fn delete_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "cluster": physical_resource_id });
    match call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.DeleteCluster"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == CLUSTER_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// One evaluated scalar property of a resource as the text an API member takes.
///
/// Read through [`scalar_text`], this module's single spelling rule, so a property a
/// template writes as a number or a boolean reaches the owning service the same way here
/// as in every other arm, and an empty string is treated as a property the template did
/// not really declare.
pub(super) fn property(resource: &StoredResource, name: &str) -> Option<String> {
    scalar_text(resource.properties.get(name)?).filter(|text| !text.is_empty())
}

/// The `Type` property both OpenSearch Serverless resource types require (a policy's
/// `encryption`/`network`, a collection's `SEARCH`/`TIMESERIES`/`VECTORSEARCH`).
///
/// A resource that declares none is refused HERE rather than with a request the owning
/// service would have to reject: the property is required by the resource type.
pub(super) fn declared_type(resource: &StoredResource) -> Result<String, Refusal> {
    property(resource, "Type").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!("{} declares no Type", resource.resource_type),
    })
}

/// The `Policy` property as the policy document text the owning service takes.
///
/// A template may spell it as the JSON text or as the document itself (an inline mapping
/// in a YAML template), which is exactly [`document_text`]'s rule, shared with the IAM
/// arm. A declaration carrying none describes no policy that could be created, so it is
/// refused here.
pub(super) fn policy_document(resource: &StoredResource) -> Result<String, Refusal> {
    resource
        .properties
        .get("Policy")
        .and_then(document_text)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!("{} declares no Policy", resource.resource_type),
        })
}

/// The cluster's configuration as Amazon ECS reports it, for drift detection.
///
/// A cluster Amazon ECS no longer holds (`ClusterNotFoundException`, an answer listing no
/// cluster, or one it reports `INACTIVE` — the status a deleted cluster keeps, which
/// `DeleteCluster` documents) was deleted.
pub(super) async fn describe_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    /// The status Amazon ECS reports for a cluster whose deletion has gone through.
    const INACTIVE: &str = "INACTIVE";

    let request = serde_json::json!({ "clusters": [physical_resource_id] });
    match call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.DescribeClusters"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(first_listed(&answer, "clusters").filter(|cluster| {
            cluster.get("status").and_then(Value::as_str) != Some(INACTIVE)
        })),
        Err(refusal) if refusal.code == CLUSTER_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The collection's configuration as OpenSearch Serverless reports it, for drift
/// detection, in the TYPE's own property vocabulary — the same correspondence the create
/// maps the other way, so an actual configuration is comparable with the template's.
///
/// `BatchGetCollection` reports a collection it no longer holds as a
/// `collectionErrorDetails` entry rather than with a refusal, so "no `collectionDetails`
/// entry" is what says the collection was deleted. A property the read does not answer is
/// absent from the document rather than guessed.
pub(super) async fn describe_collection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answer = call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.BatchGetCollection"),
        &serde_json::json!({ "ids": [physical_resource_id] }),
    )
    .await?;
    let Some(detail) = first_listed(&answer, "collectionDetails") else {
        return Ok(None);
    };
    Ok(Some(reported_as_properties(
        &detail,
        &[
            ("Name", "name"),
            ("Type", "type"),
            ("Description", "description"),
            ("StandbyReplicas", "standbyReplicas"),
        ],
    )))
}

/// The policy's configuration as OpenSearch Serverless reports it, for drift detection, in
/// the type's own property vocabulary. A policy the service no longer holds is its own
/// `ResourceNotFoundException`.
///
/// `Policy` is deliberately NOT reported: the service answers the policy as the parsed
/// document, while a template may spell it as that document or as its JSON text, so
/// comparing the two spellings would report drift where there is none.
pub(super) async fn describe_security_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "type": declared_type(resource)?,
        "name": resource.physical_resource_id,
    });
    match call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.GetSecurityPolicy"),
        &request,
    )
    .await
    {
        Ok(answer) => {
            let Some(detail) = answer.get("securityPolicyDetail") else {
                return Ok(None);
            };
            Ok(Some(reported_as_properties(
                detail,
                &[
                    ("Name", "name"),
                    ("Type", "type"),
                    ("Description", "description"),
                ],
            )))
        }
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// One answered detail document under the resource type's own property names, as
/// `(property, member)` pairs. A member the answer does not carry contributes nothing, so
/// the result reports only what the owning service really said.
pub(super) fn reported_as_properties(detail: &Value, members: &[(&str, &str)]) -> Value {
    let mut reported = Map::new();
    for (property, member) in members {
        if let Some(value) = detail.get(*member) {
            reported.insert((*property).to_string(), value.clone());
        }
    }
    Value::Object(reported)
}

/// One answered detail document as the resource's ATTRIBUTES, as `(attribute, member)`
/// pairs: the owning service's own value for each attribute the type's published return
/// values name.
///
/// A member the answer does not carry, or carries empty, contributes nothing — the resource
/// keeps whatever value CloudFormation's naming rules computed for that attribute rather
/// than taking an empty one. The ONE implementation of that reading, so two arms handed the
/// same answer cannot report it differently.
pub(super) fn reported_attributes(detail: &Value, attributes: &[(&str, &str)]) -> BTreeMap<String, String> {
    attributes
        .iter()
        .filter_map(|(attribute, member)| {
            let value = detail.get(*member).and_then(scalar_text)?;
            (!value.is_empty()).then(|| ((*attribute).to_string(), value))
        })
        .collect()
}

/// Report the refusal that failed a create SEQUENCE, having removed what the sequence had
/// already created.
///
/// A type whose resource is more than one call (a repository AND its external connections, a
/// domain AND its permissions policy) fails the whole resource when a later call is refused,
/// and what the earlier calls created is removed first: the stack records no physical id for
/// a resource whose create failed, so nothing else would ever delete it. `removal` is what
/// the owning service answered for that removal — a removal it refused TOO is named in the
/// message, so the resource left behind is diagnosable — and `noun` is what the resource is
/// called in that sentence. The wording is here once, so two arms reporting a failed
/// mid-sequence create cannot word it differently.
pub(super) fn rolled_back(refusal: Refusal, removal: Result<(), Refusal>, noun: &str) -> Refusal {
    match removal {
        Ok(()) => refusal,
        Err(failure) => Refusal {
            code: refusal.code.clone(),
            message: format!("{refusal}; the {noun} it created was left behind: {failure}"),
        },
    }
}

/// One property value with every member name inside it in LOWER CAMEL case, which is how
/// a service whose model spells its members that way names the very same members a
/// CloudFormation property names in upper camel case (`ClusterSettings: [{Name, Value}]`
/// is `settings: [{name, value}]`).
///
/// The walk is bounded by [`MAX_PROPERTY_NESTING`] for the reason that constant records:
/// the document is caller-supplied, so a deeper one is refused rather than walked. It is
/// applied only where every member inside the property is a modelled name: a property
/// carrying a MAP of caller data (a container definition's `DockerLabels`) would have its
/// keys re-cased too, and no property of the type above carries one.
///
/// `depth` is how many levels are already above this one, as in [`camel_cased`].
pub(crate) fn lower_camel_members(value: &Value, depth: usize) -> Result<Value, Refusal> {
    // No member of these types is spelled anything but the property with its initial
    // letter lower-cased, so the walk carries no exceptions.
    renamed_members(value, depth, &[])
}

/// The same walk where some of the members are NOT the property with its initial letter
/// lower-cased: `renamed` is the table of those, as (property, member) pairs, and every
/// other key is re-cased exactly as [`lower_camel_members`] does it.
///
/// That is the shape a type whose API spells one member differently needs
/// (`AWS::MSK::Cluster`'s `StorageInfo.EBSStorageInfo` against `storageInfo.\
/// ebsStorageInfo`), and the same exception-carrying arrangement [`camel_cased`] has for
/// the one map whose keys are data. A table small enough to read is what keeps this a
/// re-casing walk rather than a per-member mapping ([`mapped_members`]).
pub(super) fn renamed_members(
    value: &Value,
    depth: usize,
    renamed: &[(&str, &str)],
) -> Result<Value, Refusal> {
    if depth >= MAX_PROPERTY_NESTING {
        return Err(too_deeply_nested());
    }
    match value {
        Value::Object(members) => {
            let mut converted = Map::new();
            for (name, member) in members {
                converted.insert(
                    renamed_member(name, renamed),
                    renamed_members(member, depth + 1, renamed)?,
                );
            }
            Ok(Value::Object(converted))
        }
        Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                converted.push(renamed_members(item, depth + 1, renamed)?);
            }
            Ok(Value::Array(converted))
        }
        scalar => Ok(scalar.clone()),
    }
}

/// The API member one property name is: the one `renamed` names it, and otherwise the
/// property with its initial letter lower-cased.
pub(super) fn renamed_member(property: &str, renamed: &[(&str, &str)]) -> String {
    renamed
        .iter()
        .find(|(declared, _)| *declared == property)
        .map_or_else(
            || lower_initial(property),
            |(_, member)| (*member).to_string(),
        )
}

// ---------------------------------------------------------------------------
// Amazon SQS: AWS::SQS::Queue
// ---------------------------------------------------------------------------

/// Amazon SQS's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: sqs`, service shape `AmazonSQS`), which speaks `awsJson1_0`.
pub(super) const SQS_SIGNING_NAME: &str = "sqs";
pub(super) const SQS_TARGET_PREFIX: &str = "AmazonSQS";

/// The queue attribute naming the KMS key a queue is encrypted with. An EMPTY value is how
/// a caller takes a queue off that key ("an empty value" of `KmsMasterKeyId`), which is the
/// one reset of a dropped property Amazon SQS's own API expresses — see [`update_queue`].
pub(super) const SQS_KMS_MASTER_KEY_ID: &str = "KmsMasterKeyId";

/// The error code SQS answers for a queue url no queue answers to (`QueueDoesNotExist`,
/// the modelled error of `DeleteQueue`).
pub(super) const QUEUE_DOES_NOT_EXIST: &str = "QueueDoesNotExist";

/// Create the queue an `AWS::SQS::Queue` declares.
///
/// A queue's configuration is its ATTRIBUTES, so the call is `CreateQueue` with the
/// queue's name — the one thing CloudFormation decides, which is this resource's name and
/// the last segment of the queue url its `Ref` resolves to — and the attribute map
/// [`queue_attributes`] reads off the declared properties.
///
/// The url the create answers with is reported back as the resource's `QueueUrl`
/// attribute. It is the SAME url the type's published return values computed
/// (`https://sqs.<region>.amazonaws.com/<account>/<name>`), because both are built from
/// the one account id this emulator mints and the queue's own name — and SQS resolves a
/// `QueueUrl` by exactly that pair (`queue_name_from_url` in its own state module), so
/// the handle the stack publishes addresses the queue this call created however a client
/// reaches SQS.
///
/// The queue carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as `CreateQueue`'s own `tags` map — less the
/// `aws:cloudformation:*` ones, which `CreateQueue` refuses to a caller like the other
/// reserved-namespace APIs ([`Provisioner::applicable_tags`]).
pub(super) async fn create_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert("QueueName".to_string(), Value::String(queue_name(resource)));
    let attributes = queue_attributes(&resource.properties);
    if !attributes.is_empty() {
        request.insert("Attributes".to_string(), Value::Object(attributes));
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.CreateQueue"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(url) = reported.get("QueueUrl").and_then(Value::as_str) {
        attributes.insert("QueueUrl".to_string(), url.to_string());
    }
    // The queue was created under the name CloudFormation decided, and its url is built
    // from that name.
    Ok(Created::attributes(attributes))
}

/// Apply a changed `AWS::SQS::Queue` declaration to the queue that already exists.
///
/// A queue's configuration is its ATTRIBUTES and its TAGS, and Amazon SQS has an API for
/// changing each of them on an existing queue, so that is what this calls: one
/// `SetQueueAttributes` carrying the attributes this deployment states differently (added
/// or given another value), one `TagQueue` carrying the tags it adds or re-values, and one
/// `UntagQueue` naming the tag keys it no longer declares. A property neither revision
/// changed is not sent at all, and nothing is called where nothing changed.
///
/// The queue keeps its name, so it keeps the url its `Ref` resolves to and everything it
/// holds: the change is applied to the queue the stack created rather than replacing it.
/// The create reported the `QueueUrl` attribute and the queue still answers to it, so this
/// reports no attribute of its own.
///
/// Two changes Amazon SQS will not make to an existing queue are REFUSED rather than
/// quietly dropped, each with the refusal a client calling that API would get, so the stack
/// never reports a queue configured differently from its declaration:
///
/// * an attribute `SetQueueAttributes` does not take — `FifoQueue` is "an attribute [you
///   can] provide only during queue creation", so SQS answers `InvalidAttributeName`;
/// * a property this deployment no longer declares, which AWS resets to that attribute's
///   documented default. `SetQueueAttributes` has no way to ask for a default and the
///   defaults are Amazon SQS's own (restating them here would be a second copy of its
///   documentation), so the removal is answered with `NotImplemented` naming the property.
///
/// [`SQS_KMS_MASTER_KEY_ID`] is the one dropped property that IS applied, because Amazon
/// SQS's own API expresses that reset: an empty `KmsMasterKeyId` is how a caller takes a
/// queue off its KMS key, so a declaration that no longer names a key reaches SQS as that
/// empty value — which is what moves a queue back off SSE-KMS.
pub(super) async fn update_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let url = resource.physical_resource_id.clone();
    let before = queue_attributes(&resource.properties);
    let after = queue_attributes(declared);

    // A property the new declaration drops: nothing is called at all, so the queue is left
    // exactly as it is and the deployment reports why.
    if let Some(dropped) = before
        .keys()
        .filter(|name| !after.contains_key(*name))
        .find(|name| name.as_str() != SQS_KMS_MASTER_KEY_ID)
    {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not reset the {dropped} property of an existing \
                 {SQS_QUEUE} that an update no longer declares to Amazon SQS's documented \
                 default; the resource was left as it is."
            ),
        });
    }

    let mut changed: Map<String, Value> = after
        .iter()
        .filter(|(name, value)| before.get(*name) != Some(value))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    // The declaration no longer names a KMS key, so the queue comes off the one it is on:
    // "an empty value" of this attribute is how that is asked for.
    if before.contains_key(SQS_KMS_MASTER_KEY_ID) && !after.contains_key(SQS_KMS_MASTER_KEY_ID)
    {
        changed.insert(
            SQS_KMS_MASTER_KEY_ID.to_string(),
            Value::String(String::new()),
        );
    }
    if !changed.is_empty() {
        call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.SetQueueAttributes"),
            &serde_json::json!({ "QueueUrl": url, "Attributes": Value::Object(changed) }),
        )
        .await?;
    }

    let before_tags = queue_tags(&resource.properties);
    let after_tags = queue_tags(declared);
    let added: Map<String, Value> = after_tags
        .iter()
        .filter(|(key, value)| before_tags.get(*key) != Some(value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !added.is_empty() {
        call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.TagQueue"),
            &serde_json::json!({ "QueueUrl": url, "Tags": Value::Object(added) }),
        )
        .await?;
    }
    let removed: Vec<Value> = before_tags
        .keys()
        .filter(|key| !after_tags.contains_key(*key))
        .map(|key| Value::String(key.clone()))
        .collect();
    if !removed.is_empty() {
        call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.UntagQueue"),
            &serde_json::json!({ "QueueUrl": url, "TagKeys": Value::Array(removed) }),
        )
        .await?;
    }

    Ok(Created::default())
}

/// Delete the queue a stack created. A queue SQS no longer has (`QueueDoesNotExist`) is
/// already in the state the delete asks for.
///
/// The queue is addressed by its url, which is this resource's physical id — "Ref returns
/// the queue URL" for this type.
pub(super) async fn delete_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "QueueUrl": physical_resource_id });
    match call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.DeleteQueue"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == QUEUE_DOES_NOT_EXIST => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The queue's configuration as Amazon SQS reports it, for drift detection.
///
/// A queue SQS no longer has (`QueueDoesNotExist`) was deleted. A queue's configuration IS
/// its attribute map, named identically to this type's properties ([`queue_attributes`]),
/// so the attributes are the actual configuration compared against the declared one and a
/// queue whose visibility timeout was changed outside the stack is MODIFIED.
pub(super) async fn describe_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "QueueUrl": physical_resource_id,
        "AttributeNames": ["All"],
    });
    match call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.GetQueueAttributes"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_member(&answer, "Attributes"))),
        Err(refusal) if refusal.code == QUEUE_DOES_NOT_EXIST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME of the queue a resource record describes: the `QueueName` its type's
/// published return values carry, which is the name the template asked for or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated — never the physical id,
/// which for this type is the queue's url.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to
/// the last segment of that url, which is the queue's name.
pub(super) fn queue_name(resource: &StoredResource) -> String {
    if let Some(name) = resource.attributes.get("QueueName") {
        return name.clone();
    }
    resource
        .physical_resource_id
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Attributes` map an `AWS::SQS::Queue`'s declared properties are, as `CreateQueue`
/// takes them.
///
/// This type's properties ARE the queue's own attributes, named identically on both sides
/// (`VisibilityTimeout`, `MessageRetentionPeriod`, `DelaySeconds`, `FifoQueue`,
/// `RedrivePolicy`, …): CloudFormation's property names for the type are the attribute
/// names `CreateQueue`/`SetQueueAttributes` document. So every property is passed through
/// under its own name except the two that are not attributes — `QueueName`, which is the
/// create's own member and the one value CloudFormation decides, and `Tags`, which the
/// create takes as its own `tags` member ([`queue_tags`]).
///
/// An attribute value is a STRING on the wire, so a number or a boolean the template
/// wrote as such is rendered as the text it is, and the policy attributes — documented
/// "as a JSON object" — as the compact JSON of the object declared. A property SQS does
/// not know is not filtered out here: the owning service's own validation is what judges
/// an attribute, and its refusal leaves the stack `CREATE_FAILED` rather than a queue
/// configured differently from the template.
pub(super) fn queue_attributes(properties: &Value) -> Map<String, Value> {
    let Some(properties) = properties.as_object() else {
        return Map::new();
    };
    properties
        .iter()
        .filter(|(property, _)| property.as_str() != "QueueName" && property.as_str() != "Tags")
        .map(|(property, value)| (property.clone(), Value::String(attribute_text(value))))
        .collect()
}

/// One value as the wire carries it: a string stays itself, every other JSON value is its
/// own compact JSON (`30`, `true`, `{"maxReceiveCount":3}`).
///
/// The ONE spelling of that, shared by the queue attributes it was written for and by every
/// other member whose value a template may declare as a document while the called operation
/// takes its TEXT — an api's inline OpenAPI definition among them
/// ([`import_apigatewayv2_api`]). The rule itself is stated once in the shared crate
/// ([`::service_common::json_wire_values::value_text`]), so this crate and the services that
/// take such a member cannot disagree about what a number or an object spells as.
pub(crate) fn attribute_text(value: &Value) -> String {
    ::service_common::json_wire_values::value_text(value)
}

/// The `tags` map an `AWS::SQS::Queue`'s `Tags` property is.
///
/// CloudFormation's tag property is a LIST of `{Key, Value}` pairs and `CreateQueue`
/// takes a map, which is the conversion [`declared_tags`] and [`tags_as_map`] do.
pub(super) fn queue_tags(properties: &Value) -> Map<String, Value> {
    tags_as_map(&declared_tags(properties))
}

// ---------------------------------------------------------------------------
// Amazon SQS: AWS::SQS::QueuePolicy and AWS::SQS::QueueInlinePolicy
// ---------------------------------------------------------------------------

/// A queue's access policy is an ATTRIBUTE of the queue — `Policy`, "the queue's policy. A
/// valid Amazon Web Services policy" — so both policy resource types are deployed through
/// the one Amazon SQS call that writes an attribute of an existing queue, exactly as
/// [`update_queue`] applies every other changed attribute.
pub(super) const SQS_SET_QUEUE_ATTRIBUTES: &str = "SetQueueAttributes";

/// The value that REMOVES the attribute a policy resource wrote: Amazon SQS takes an empty
/// string for an attribute whose form admits text of any length, which is how a queue
/// stops carrying a policy at all. It is what a deleted policy resource — or one whose
/// declaration no longer names a queue it used to — leaves the queue with.
pub(super) const NO_POLICY: &str = "";

/// The queue urls a policy resource applies its document to.
///
/// `AWS::SQS::QueuePolicy` names them in `Queues`, "the URLs of the queues to which you
/// want to add the policy"; `AWS::SQS::QueueInlinePolicy` names the one queue it belongs to
/// in `Queue`. Both are read here, so the two types differ only in which property carries
/// the queue and not in how the document reaches it.
pub(super) fn policy_queue_urls(properties: &Value) -> Vec<String> {
    let mut urls: Vec<String> = properties
        .get("Queues")
        .and_then(Value::as_array)
        .map(|declared| {
            declared
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if let Some(queue) = properties.get("Queue").and_then(Value::as_str) {
        urls.push(queue.to_string());
    }
    urls
}

/// The document a policy resource declares, as the `Policy` attribute carries it.
///
/// A template may spell it as an inline object or as the JSON text, which is
/// [`document_text`]'s rule, shared with the IAM and S3 policy arms; a declaration carrying
/// none describes no policy that could be applied, so it is refused here rather than with a
/// request Amazon SQS would have to reject.
pub(super) fn declared_policy_document(properties: &Value, resource_type: &str) -> Result<String, Refusal> {
    properties
        .get("PolicyDocument")
        .and_then(document_text)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!("{resource_type} declares no PolicyDocument"),
        })
}

/// Write the policy a `AWS::SQS::QueuePolicy` / `AWS::SQS::QueueInlinePolicy` declares onto
/// each queue it names.
///
/// The policy is not a resource of Amazon SQS's own: it is the `Policy` attribute of the
/// queues the declaration names, so this resource's deployment IS that attribute write, and
/// a later `GetQueueAttributes` reports the rendered document. CloudFormation decides this
/// resource's physical id (its generated name), so nothing is reported back.
pub(super) async fn put_queue_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let document = declared_policy_document(&resource.properties, &resource.resource_type)?;
    for url in policy_queue_urls(&resource.properties) {
        set_queue_policy(ctx, region, &url, &document).await?;
    }
    Ok(Created::default())
}

/// Apply a CHANGED policy declaration: every queue the new declaration names carries the
/// new document, and a queue it no longer names stops carrying one — which is how a stack
/// update REVOKES a grant it used to make.
pub(super) async fn update_queue_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let document = declared_policy_document(declared, &resource.resource_type)?;
    let named = policy_queue_urls(declared);
    for url in policy_queue_urls(&resource.properties) {
        if !named.contains(&url) {
            set_queue_policy(ctx, region, &url, NO_POLICY).await?;
        }
    }
    for url in &named {
        set_queue_policy(ctx, region, url, &document).await?;
    }
    Ok(Created::default())
}

/// Remove the policy a deleted policy resource had written: the queues it named stop
/// carrying a document, which is the state the delete asks for.
pub(super) async fn delete_queue_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    for url in policy_queue_urls(&resource.properties) {
        set_queue_policy(ctx, region, &url, NO_POLICY).await?;
    }
    Ok(())
}

/// One `SetQueueAttributes` writing the `Policy` attribute of the queue at `url`.
///
/// A queue Amazon SQS no longer holds (`QueueDoesNotExist`) carries no attribute to write:
/// a stack whose queue was deleted before the policy that named it is already in the state
/// the deployment asks for.
pub(super) async fn set_queue_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    url: &str,
    document: &str,
) -> Result<(), Refusal> {
    match call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.{SQS_SET_QUEUE_ATTRIBUTES}"),
        &serde_json::json!({ "QueueUrl": url, "Attributes": { "Policy": document } }),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == QUEUE_DOES_NOT_EXIST => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The policy the queues a policy resource names carry NOW, for drift detection.
///
/// The resource's configuration is the document those queues hold, so the actual
/// configuration is read from Amazon SQS's own `Policy` attribute. No queue carrying one
/// means the resource's effect is gone, which is the `DELETED` drift status; a document a
/// queue carries is reported as the attribute's text, which [`super::drift`] compares
/// against the declared object by parsing it.
pub(super) async fn describe_queue_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let urls = policy_queue_urls(&resource.properties);
    let mut carried: Option<String> = None;
    for url in &urls {
        let request = serde_json::json!({ "QueueUrl": url, "AttributeNames": ["Policy"] });
        let answer = match call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.GetQueueAttributes"),
            &request,
        )
        .await
        {
            Ok(answer) => answer,
            // A queue that is gone carries no policy, which is what this read reports.
            Err(refusal) if refusal.code == QUEUE_DOES_NOT_EXIST => continue,
            Err(refusal) => return Err(refusal),
        };
        if let Some(document) = answer
            .get("Attributes")
            .and_then(|attributes| attributes.get("Policy"))
            .and_then(Value::as_str)
        {
            carried = Some(document.to_string());
            break;
        }
    }
    let Some(document) = carried else {
        return Ok(None);
    };
    // Reported under the same property names the declaration uses, so the comparison is
    // between the declared document and the one the queues hold.
    let mut reported = Map::new();
    if resource.properties.get("Queues").is_some() {
        reported.insert(
            "Queues".to_string(),
            Value::Array(urls.iter().map(|url| Value::String(url.clone())).collect()),
        );
    }
    if let Some(queue) = resource.properties.get("Queue") {
        reported.insert("Queue".to_string(), queue.clone());
    }
    reported.insert("PolicyDocument".to_string(), Value::String(document));
    Ok(Some(Value::Object(reported)))
}

// ---------------------------------------------------------------------------
// The tags a provisioned resource carries
// ---------------------------------------------------------------------------

/// The tags ONE provisioned resource carries: the ones its own declaration asks for, then
/// the ones CloudFormation propagates to it under every key that declaration does not use.
///
/// The rule: a resource CloudFormation creates carries the stack-level tags plus the three
/// `aws:cloudformation:*` tags ([`super::provisioning::StackProvenance::tags_for`]), and a
/// key the resource's own `Tags` property states keeps the value that property gives it —
/// the declaration is the more specific statement about that one resource. The three
/// CloudFormation records itself are the exception, exactly as they are in `tags_for`: they
/// are CloudFormation's to set, so a declaration spelling one of them does not displace it.
/// The result is ORDERED (declared first, in declaration order), so the same declaration
/// builds the same request however the maps behind it happen to iterate.
pub(crate) fn tags_of(
    properties: &Value,
    propagated: &[(String, String)],
) -> Vec<(String, String)> {
    let mut tags = declared_tags(properties);
    for (key, value) in propagated {
        if key.is_empty() {
            continue;
        }
        match tags.iter().position(|(held, _)| held == key) {
            Some(index) if is_cloudformation_tag(key) => tags[index].1 = value.clone(),
            Some(_) => {}
            None => tags.push((key.clone(), value.clone())),
        }
    }
    tags
}

/// Whether `key` is one of the three tags CloudFormation records on every resource it
/// provisions, which no declaration of its own may displace.
pub(super) fn is_cloudformation_tag(key: &str) -> bool {
    super::provisioning::is_stack_provenance_tag(key)
}

/// The tags a resource's own declaration asks for, in the order it writes them.
///
/// CloudFormation spells a type's `Tags` property either as the `[{Key, Value}]` list most
/// types publish or as the MAP of tag key to value a few do
/// (`AWS::FIS::ExperimentTemplate`), and both are read here, so a declaration reaches the
/// owning service whichever spelling the type uses. A tag with no key is not a tag and is
/// dropped rather than sent as an empty one; a value that is not a string is sent as the
/// text it is, since a tag value is a string on every wire that takes one.
///
/// A key the LIST form states twice is one tag, not two — a tag set is a map from key to
/// value, so a repeated key keeps its first position with the later value, which is the
/// same rule [`super::provisioning::StackProvenance::tags_for`] applies to a stack's own
/// tags. (The map form cannot state a key twice.)
pub(super) fn declared_tags(properties: &Value) -> Vec<(String, String)> {
    let declared: Vec<(String, String)> = match properties.get("Tags") {
        Some(Value::Array(tags)) => tags
            .iter()
            .filter_map(|tag| {
                let key = tag.get("Key").and_then(Value::as_str)?;
                if key.is_empty() {
                    return None;
                }
                Some((
                    key.to_string(),
                    tag.get("Value").map(attribute_text).unwrap_or_default(),
                ))
            })
            .collect(),
        Some(Value::Object(tags)) => tags
            .iter()
            .filter(|(key, _)| !key.is_empty())
            .map(|(key, value)| (key.clone(), attribute_text(value)))
            .collect(),
        _ => Vec::new(),
    };
    let mut tags: Vec<(String, String)> = Vec::with_capacity(declared.len());
    for (key, value) in declared {
        match tags.iter().position(|(held, _)| *held == key) {
            Some(index) => tags[index].1 = value,
            None => tags.push((key, value)),
        }
    }
    tags
}

/// AWS's reserved tag-key namespace: "the `aws:` prefix is reserved for AWS use", so it is
/// the namespace a caller's own request may not write into. Matched case-insensitively, as
/// the services that refuse it match it.
pub(super) const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
pub(super) fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The member names a service's own `Tag` shape spells a tag's key and value with: upper
/// camel on Amazon Data Firehose and Amazon ECR Public (`Key`/`Value`), lower camel on
/// Amazon ECS and CodePipeline (`key`/`value`), which is the one difference between the
/// list each of them takes.
pub(super) const UPPER_CASED_TAG: (&str, &str) = ("Key", "Value");
pub(super) const LOWER_CASED_TAG: (&str, &str) = ("key", "value");

/// A tag set as the list of `Tag` structures a creating API takes, with each structure's
/// members spelled the way that service's model spells them ([`UPPER_CASED_TAG`],
/// [`LOWER_CASED_TAG`]).
pub(super) fn tags_as_list(tags: &[(String, String)], members: (&str, &str)) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| {
                let mut tag = Map::new();
                tag.insert(members.0.to_string(), Value::String(key.clone()));
                tag.insert(members.1.to_string(), Value::String(value.clone()));
                Value::Object(tag)
            })
            .collect(),
    )
}

/// A tag set as the MAP of tag key to value a creating API that takes one takes (Amazon
/// SQS's `tags`, AWS FIS's `tags`).
pub(crate) fn tags_as_map(tags: &[(String, String)]) -> Map<String, Value> {
    tags.iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect()
}

/// The two halves of bringing a resource's tags to the set this deployment gives it: the
/// tags to WRITE — one the resource does not carry at all, or carries with another value —
/// and the KEYS to drop, which are the ones it carries that the set no longer states.
///
/// A tag set is a map from key to value, and that arithmetic is the same whichever service
/// owns the resource: only the calls that apply the two halves are each service's own. So it
/// is written once here and every re-tagging arm ([`Provisioner::retag`]) asks it, which is
/// also what makes those arms idempotent — a resource whose tags already agree with the set
/// yields two empty halves and nothing is called about it.
pub(super) fn tag_delta(
    held: &[(String, String)],
    desired: &[(String, String)],
) -> (Vec<(String, String)>, Vec<String>) {
    let added: Vec<(String, String)> = desired
        .iter()
        .filter(|(key, value)| {
            held.iter()
                .find(|(held_key, _)| held_key == key)
                .is_none_or(|(_, held_value)| held_value != value)
        })
        .cloned()
        .collect();
    let removed: Vec<String> = held
        .iter()
        .filter(|(key, _)| !desired.iter().any(|(wanted, _)| wanted == key))
        .map(|(key, _)| key.clone())
        .collect();
    (added, removed)
}

/// One tag-key list as the JSON array of strings an untagging call takes, for a service
/// whose wire carries it as one ([`tag_delta`]'s second half).
pub(super) fn tag_keys_as_list(keys: &[String]) -> Value {
    Value::Array(keys.iter().map(|key| Value::String(key.clone())).collect())
}

// ---------------------------------------------------------------------------
// Amazon SNS: AWS::SNS::Topic
// ---------------------------------------------------------------------------

/// Amazon SNS's signing name and API version, from its model (`sigv4Name: sns`, service
/// shape `AmazonSimpleNotificationService`, `version: 2010-03-31`), which speaks
/// `awsQuery` — so its calls ride on [`call_query`] rather than on [`call_json_rpc`].
pub(super) const SNS_SIGNING_NAME: &str = "sns";
pub(super) const SNS_API_VERSION: &str = "2010-03-31";

/// The error code SNS answers for a topic ARN no topic answers to: `NotFoundException`'s
/// own `awsQueryError` code, which is what an awsQuery client branches on.
pub(super) const NOT_FOUND: &str = "NotFound";

/// The properties of an `AWS::SNS::Topic` that are NOT topic attributes, each a member of
/// `CreateTopic` in its own right or a call of its own:
///
/// * `TopicName` is the one value CloudFormation decides, and the create's `Name` member;
/// * `Tags` is the create's own tag list;
/// * `DataProtectionPolicy` is the create's own member ("The body of the policy document
///   you want to use for this topic");
/// * `Subscription` is a list of subscriptions, which are `Subscribe` calls on the topic
///   once it exists.
pub(super) const NON_ATTRIBUTE_PROPERTIES: &[&str] =
    &["TopicName", "Tags", "DataProtectionPolicy", "Subscription"];

/// The property whose entries are the per-protocol delivery-status attributes.
pub(super) const DELIVERY_STATUS_LOGGING: &str = "DeliveryStatusLogging";

/// How a `DeliveryStatusLogging` entry's `Protocol` is spelled in the delivery-status
/// attribute names SNS documents: a `sqs` entry's role ARN is the topic's
/// `SQSSuccessFeedbackRoleArn`, a `http/s` entry's its `HTTPSuccessFeedbackRoleArn`.
///
/// Source: the `Protocol` value set AWS's resource reference publishes for
/// `AWS::SNS::Topic`'s `LoggingConfig`, against the attribute names
/// `SetTopicAttributes` documents. A protocol outside this set is passed through as the
/// template spells it, so SNS's own validation is what refuses it rather than a name
/// invented here.
pub(super) const LOGGING_PROTOCOL_PREFIXES: &[(&str, &str)] = &[
    ("http/s", "HTTP"),
    ("sqs", "SQS"),
    ("lambda", "Lambda"),
    ("firehose", "Firehose"),
    ("application", "Application"),
];

/// The members of one `LoggingConfig`, named identically on both sides once the protocol's
/// prefix is in front of them. `Protocol` itself is the prefix, not a member.
pub(super) const LOGGING_CONFIG_MEMBERS: &[&str] = &[
    "SuccessFeedbackRoleArn",
    "SuccessFeedbackSampleRate",
    "FailureFeedbackRoleArn",
];

/// Create the topic an `AWS::SNS::Topic` declares, and the subscriptions it declares on it.
///
/// A topic's configuration is its ATTRIBUTES, so the call is `CreateTopic` with the topic's
/// name — the one thing CloudFormation decides, which is the last segment of the ARN its
/// `Ref` resolves to — and the attribute map [`topic_attributes`] reads off the declared
/// properties.
///
/// The ARN the create answers with is reported back as the resource's `TopicArn`
/// attribute. It is the SAME ARN the type's published return values computed
/// (`arn:aws:sns:<region>:<account>:<name>`), because both are built from the one account
/// id this emulator mints and the topic's own name — and SNS keys a topic by exactly that
/// ARN, so the handle the stack publishes addresses the topic this call created however a
/// client reaches SNS.
///
/// The topic carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as `CreateTopic`'s own `Tags` list — so `ListTagsForResource`
/// on the ARN the stack publishes answers with the stack's tags and the
/// `aws:cloudformation:*` ones.
pub(super) async fn create_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = vec![("Name".to_string(), topic_name(resource))];
    for (index, (name, value)) in topic_attributes(&resource.properties).iter().enumerate() {
        let entry = index + 1;
        parameters.push((format!("Attributes.entry.{entry}.key"), name.clone()));
        parameters.push((format!("Attributes.entry.{entry}.value"), value.clone()));
    }
    // Amazon SNS's own `Tag` shape carries no `@xmlName` on its list member, so its tag
    // list is indexed under the Smithy default element ([`push_tag_list`]).
    push_tag_list(
        &mut parameters,
        "Tags",
        "member",
        &tags_of(&resource.properties, propagated),
    );
    if let Some(policy) = resource.properties.get("DataProtectionPolicy") {
        parameters.push(("DataProtectionPolicy".to_string(), attribute_text(policy)));
    }
    let answered = call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "CreateTopic",
        &parameters,
    )
    .await?;
    let arn = xml_element(&answered, "TopicArn")
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon SNS answered CreateTopic without a TopicArn".to_string(),
        })?;

    // "The SNS subscriptions (endpoints) for this topic" — declared inline on the topic,
    // so they are subscriptions of the topic this call just created.
    for subscription in declared_subscriptions(&resource.properties) {
        let mut parameters = vec![("TopicArn".to_string(), arn.clone())];
        parameters.extend(subscription);
        let subscribed = call_query(
            ctx,
            SNS_SIGNING_NAME,
            region,
            SNS_API_VERSION,
            "Subscribe",
            &parameters,
        )
        .await;
        if let Err(refusal) = subscribed {
            // The resource the template declares is the topic AND the subscriptions on
            // it, so a subscription SNS refuses fails the resource. The stack records no
            // physical id for a resource whose create failed, so nothing would ever
            // delete the topic this call already made: it is deleted here, before the
            // refusal propagates, and what the stack reports is SNS's own refusal.
            delete_topic(ctx, region, &arn)
                .await
                .map_err(|failure| Refusal {
                    code: refusal.code.clone(),
                    message: format!(
                        "{refusal}; the topic it created was left behind: {failure}"
                    ),
                })?;
            return Err(refusal);
        }
    }

    let mut attributes = BTreeMap::new();
    attributes.insert("TopicArn".to_string(), arn);
    Ok(Created::attributes(attributes))
}

/// What a topic attribute goes back to when the declaration stops carrying it: the value
/// Amazon SNS documents as that attribute's absent state.
///
/// A policy-valued attribute is REMOVED by an empty value, a topic comes off its KMS key the
/// same way, the display name a topic was never given is the empty one, and the three
/// switches go back to the defaults `SetTopicAttributes` documents for them ("By default
/// TracingConfig is set to PassThrough", "By default SignatureVersion is set to 1", content
/// based deduplication being off). An attribute with no documented absent value — a
/// delivery-status feedback role or sample rate — is left as it is rather than written a
/// value SNS does not define, exactly as a subscription's own
/// [`SUBSCRIPTION_ATTRIBUTE_CLEARED`] leaves one.
pub(super) const TOPIC_ATTRIBUTE_CLEARED: &[(&str, &str)] = &[
    ("ArchivePolicy", ""),
    ("ContentBasedDeduplication", "false"),
    ("DeliveryPolicy", ""),
    ("DisplayName", ""),
    ("KmsMasterKeyId", ""),
    ("Policy", ""),
    ("SignatureVersion", "1"),
    ("TracingConfig", "PassThrough"),
];

/// The Amazon SNS call that writes ONE attribute of a topic that already exists.
pub(super) const SNS_SET_TOPIC_ATTRIBUTES: &str = "SetTopicAttributes";

/// Apply a CHANGED `AWS::SNS::Topic` declaration to the topic that already exists.
///
/// A topic's configuration is its ATTRIBUTES ([`topic_attributes`]), and Amazon SNS has one
/// call that writes an attribute of a live topic — "allows a topic owner to set an attribute
/// of the topic to a new value" — so that is what this makes, once per attribute whose
/// declared value moved; an attribute the declaration stopped carrying goes back to its
/// documented absent value ([`TOPIC_ATTRIBUTE_CLEARED`]). An attribute neither revision
/// changed is not sent at all, and nothing is called where nothing changed.
///
/// The topic keeps its name, so it keeps the ARN its `Ref` resolves to, its subscribers and
/// everything else it holds: the change is applied to the topic the stack created rather
/// than replacing it. The create reported the `TopicArn` attribute and the topic still
/// answers to it, so this reports no attribute of its own.
///
/// The declaration's `DataProtectionPolicy` is no attribute: it is a member of the create
/// and of the one call that writes it on a topic that exists, so a changed document travels
/// through that call — and SNS's own answer to it is what the deployment reports.
///
/// The inline `Subscription` list is not a property of the topic either: each entry is a
/// subscription of Amazon SNS's own, which `Subscribe` creates and `Unsubscribe` removes
/// against the subscription ARN that answered — an identifier this type's record does not
/// carry. A declaration that changes that list is therefore REFUSED rather than quietly
/// dropped, so the stack never reports a topic whose subscribers differ from its
/// declaration; `AWS::SNS::Subscription` is the type whose own resources this emulator
/// reconciles.
pub(super) async fn update_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let arn = resource.physical_resource_id.clone();
    if declared_subscriptions(declared) != declared_subscriptions(&resource.properties) {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not change the inline Subscription list of an existing \
                 {SNS_TOPIC}; the resource was left as it is."
            ),
        });
    }
    let held = topic_attributes(&resource.properties);
    let wanted = topic_attributes(declared);
    for (name, value) in &wanted {
        if held.get(name) != Some(value) {
            set_topic_attribute(ctx, region, &arn, name, value).await?;
        }
    }
    for name in held.keys() {
        if wanted.contains_key(name) {
            continue;
        }
        let Some((_, absent)) = TOPIC_ATTRIBUTE_CLEARED
            .iter()
            .find(|(attribute, _)| attribute == name)
        else {
            continue;
        };
        set_topic_attribute(ctx, region, &arn, name, absent).await?;
    }
    let policy = |properties: &Value| {
        properties
            .get("DataProtectionPolicy")
            .map(attribute_text)
            .unwrap_or_default()
    };
    if policy(declared) != policy(&resource.properties) {
        call_query(
            ctx,
            SNS_SIGNING_NAME,
            region,
            SNS_API_VERSION,
            "PutDataProtectionPolicy",
            &[
                ("ResourceArn".to_string(), arn.clone()),
                ("DataProtectionPolicy".to_string(), policy(declared)),
            ],
        )
        .await?;
    }
    Ok(Created::default())
}

/// Write one attribute of a topic that already exists, through the one Amazon SNS call that
/// takes a single attribute and its value.
pub(super) async fn set_topic_attribute(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    topic_arn: &str,
    name: &str,
    value: &str,
) -> Result<(), Refusal> {
    call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        SNS_SET_TOPIC_ATTRIBUTES,
        &[
            ("TopicArn".to_string(), topic_arn.to_string()),
            ("AttributeName".to_string(), name.to_string()),
            ("AttributeValue".to_string(), value.to_string()),
        ],
    )
    .await
    .map(|_| ())
}

/// Bring the tags of the topic a resource record names to the ones this deployment gives
/// it, through Amazon SNS's own `TagResource` and `UntagResource`.
///
/// The set is the declaration's own `Tags` beside the ones CloudFormation propagates
/// ([`tags_of`]) — the same set the CREATE applied — so a key outside it is one this
/// deployment no longer gives the resource and is removed, and one in it with another value
/// is re-valued. Where the two sets already agree nothing is called at all, which is what
/// makes this safe to ask on every deployment.
///
/// A topic SNS no longer has (`NotFound`) has no tags to reconcile.
pub(super) async fn retag_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let arn = resource.physical_resource_id.clone();
    let listed = match call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "ListTagsForResource",
        &[("ResourceArn".to_string(), arn.clone())],
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == NOT_FOUND => return Ok(()),
        Err(refusal) => return Err(refusal),
    };
    // Amazon SNS's `Tag` carries a `Key` and a `Value` on every entry of the list its tag
    // read answers with, so the two members are read as parallel lists and paired. (Named
    // through its module rather than imported, to keep this arm out of the import list every
    // other arm shares.)
    let held: Vec<(String, String)> = super::provisioning::xml_elements(&listed, "Key")
        .into_iter()
        .zip(super::provisioning::xml_elements(&listed, "Value"))
        .collect();
    let desired = tags_of(declared, propagated);
    let (added, removed) = tag_delta(&held, &desired);
    if !added.is_empty() {
        let mut parameters = vec![("ResourceArn".to_string(), arn.clone())];
        push_tag_list(&mut parameters, "Tags", "member", &added);
        call_query(
            ctx,
            SNS_SIGNING_NAME,
            region,
            SNS_API_VERSION,
            "TagResource",
            &parameters,
        )
        .await?;
    }
    if !removed.is_empty() {
        let mut parameters = vec![("ResourceArn".to_string(), arn)];
        for (index, key) in removed.iter().enumerate() {
            let position = index + 1;
            parameters.push((format!("TagKeys.member.{position}"), key.clone()));
        }
        call_query(
            ctx,
            SNS_SIGNING_NAME,
            region,
            SNS_API_VERSION,
            "UntagResource",
            &parameters,
        )
        .await?;
    }
    Ok(())
}

/// Delete the topic a stack created. A topic SNS no longer has (`NotFound`) is already in
/// the state the delete asks for.
///
/// The topic is addressed by its ARN, which is this resource's physical id — "Ref returns
/// the topic ARN" for this type.
pub(super) async fn delete_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![("TopicArn".to_string(), physical_resource_id.to_string())];
    match call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "DeleteTopic",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The topic's configuration as Amazon SNS reports it, for drift detection.
///
/// A topic SNS no longer has (`NotFound`) was deleted. A topic's configuration IS its
/// attribute map, named identically to this type's properties ([`topic_attributes`]), so
/// the attributes are the actual configuration compared against the declared one and a
/// topic whose display name was changed outside the stack is MODIFIED. The map the query
/// protocol serializes as `<entry><key/><value/></entry>` is read back as the JSON object
/// the comparison takes.
pub(super) async fn describe_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![("TopicArn".to_string(), physical_resource_id.to_string())];
    match call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "GetTopicAttributes",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_entry_map(&answered, "Attributes")))),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME of the topic a resource record describes: the `TopicName` its type's published
/// return values carry, which is the name the template asked for or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated — never the physical id,
/// which for this type is the topic's ARN.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to the
/// last segment of that ARN, which is the topic's name.
pub(super) fn topic_name(resource: &StoredResource) -> String {
    if let Some(name) = resource.attributes.get("TopicName") {
        return name.clone();
    }
    resource
        .physical_resource_id
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// Amazon SNS: AWS::SNS::TopicPolicy and AWS::SNS::TopicInlinePolicy
// ---------------------------------------------------------------------------

/// The topic attribute both policy types write: `Policy`, "the JSON serialization of the
/// topic's access control policy" (`GetTopicAttributes`) — so a policy resource's
/// deployment IS that attribute write, through the one Amazon SNS call that writes an
/// attribute of an existing topic ([`set_topic_attribute`]), exactly as [`update_topic`]
/// applies every other changed attribute.
pub(super) const TOPIC_POLICY_ATTRIBUTE: &str = "Policy";

/// The topic ARNs a policy resource applies its document to.
///
/// `AWS::SNS::TopicPolicy` names them in `Topics`, the topics to add the policy to;
/// `AWS::SNS::TopicInlinePolicy` names the one topic it belongs to in `TopicArn`. Both are
/// read here, so the two types differ only in which property carries the topic and not in
/// how the document reaches it.
pub(super) fn policy_topic_arns(properties: &Value) -> Vec<String> {
    let mut arns: Vec<String> = properties
        .get("Topics")
        .and_then(Value::as_array)
        .map(|declared| {
            declared
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if let Some(topic) = properties.get("TopicArn").and_then(Value::as_str) {
        arns.push(topic.to_string());
    }
    arns
}

/// Write the policy an `AWS::SNS::TopicPolicy` / `AWS::SNS::TopicInlinePolicy` declares onto
/// each topic it names.
///
/// The policy is not a resource of Amazon SNS's own: it is the `Policy` attribute of the
/// topics the declaration names, so a later `GetTopicAttributes` reports the rendered
/// document in place of the default policy the topic came up with — which is what lets
/// another service's publish to the topic be authorized by the grant the template declared.
/// CloudFormation decides this resource's physical id (its generated name), so nothing is
/// reported back.
pub(super) async fn put_topic_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let document = declared_policy_document(&resource.properties, &resource.resource_type)?;
    for arn in policy_topic_arns(&resource.properties) {
        write_topic_policy(ctx, region, &arn, &document).await?;
    }
    Ok(Created::default())
}

/// Apply a CHANGED policy declaration: every topic the new declaration names carries the
/// new document, and a topic it no longer names stops carrying one — which is how a stack
/// update REVOKES a grant it used to make.
pub(super) async fn update_topic_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let document = declared_policy_document(declared, &resource.resource_type)?;
    let named = policy_topic_arns(declared);
    for arn in policy_topic_arns(&resource.properties) {
        if !named.contains(&arn) {
            write_topic_policy(ctx, region, &arn, NO_POLICY).await?;
        }
    }
    for arn in &named {
        write_topic_policy(ctx, region, arn, &document).await?;
    }
    Ok(Created::default())
}

/// Remove the policy a deleted policy resource had written: the topics it named stop
/// carrying a document, which is the state the delete asks for.
pub(super) async fn delete_topic_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    for arn in policy_topic_arns(&resource.properties) {
        write_topic_policy(ctx, region, &arn, NO_POLICY).await?;
    }
    Ok(())
}

/// One `SetTopicAttributes` writing the `Policy` attribute of the topic `arn` names.
///
/// A topic Amazon SNS no longer holds (`NotFound`) carries no attribute to write: a stack
/// whose topic was deleted before the policy that named it is already in the state the
/// deployment asks for.
pub(super) async fn write_topic_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
    document: &str,
) -> Result<(), Refusal> {
    match set_topic_attribute(ctx, region, arn, TOPIC_POLICY_ATTRIBUTE, document).await {
        Ok(()) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The policy the topics a policy resource names carry NOW, for drift detection.
///
/// The resource's configuration is the document those topics hold, so the actual
/// configuration is read from Amazon SNS's own `Policy` attribute; a topic that is gone
/// carries none, and no topic carrying one means the resource's effect is gone, which is the
/// `DELETED` drift status. The document is reported as the attribute's text, which
/// [`super::drift`] compares against the declared object by parsing it.
pub(super) async fn describe_topic_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let arns = policy_topic_arns(&resource.properties);
    let mut carried: Option<String> = None;
    for arn in &arns {
        let answered = match call_query(
            ctx,
            SNS_SIGNING_NAME,
            region,
            SNS_API_VERSION,
            "GetTopicAttributes",
            &[("TopicArn".to_string(), arn.clone())],
        )
        .await
        {
            Ok(answered) => answered,
            Err(refusal) if refusal.code == NOT_FOUND => continue,
            Err(refusal) => return Err(refusal),
        };
        if let Some(document) = xml_entry_map(&answered, "Attributes")
            .get(TOPIC_POLICY_ATTRIBUTE)
            .and_then(Value::as_str)
            .filter(|document| !document.is_empty())
        {
            carried = Some(document.to_string());
            break;
        }
    }
    let Some(document) = carried else {
        return Ok(None);
    };
    // Reported under the same property names the declaration uses, so the comparison is
    // between the declared document and the one the topics hold.
    let mut reported = Map::new();
    if resource.properties.get("Topics").is_some() {
        reported.insert(
            "Topics".to_string(),
            Value::Array(arns.iter().map(|arn| Value::String(arn.clone())).collect()),
        );
    }
    if let Some(topic) = resource.properties.get("TopicArn") {
        reported.insert("TopicArn".to_string(), topic.clone());
    }
    reported.insert("PolicyDocument".to_string(), Value::String(document));
    Ok(Some(Value::Object(reported)))
}

/// The `Attributes` map an `AWS::SNS::Topic`'s declared properties are, as `CreateTopic`
/// takes them.
///
/// This type's properties ARE the topic's own attributes, named identically on both sides
/// (`DisplayName`, `FifoTopic`, `KmsMasterKeyId`, `SignatureVersion`, `TracingConfig`,
/// `ArchivePolicy`, `ContentBasedDeduplication`, `FifoThroughputScope`): CloudFormation's
/// property names for the type are the attribute names
/// `CreateTopic`/`SetTopicAttributes` document. So every property is passed through under
/// its own name except the ones that are not attributes ([`NON_ATTRIBUTE_PROPERTIES`]) and
/// `DeliveryStatusLogging`, whose entries are the per-protocol attributes
/// [`delivery_status_attributes`] names.
///
/// An attribute value is a STRING on the wire, so a number or a boolean the template wrote
/// as such is rendered as the text it is, and the policy attributes — documented as JSON
/// objects — as the compact JSON of the object declared. A property SNS does not know is
/// not filtered out here: the owning service's own validation is what judges an attribute,
/// and its refusal leaves the stack `CREATE_FAILED` rather than a topic configured
/// differently from the template.
pub(super) fn topic_attributes(properties: &Value) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    let Some(properties) = properties.as_object() else {
        return attributes;
    };
    for (property, value) in properties {
        if NON_ATTRIBUTE_PROPERTIES.contains(&property.as_str()) {
            continue;
        }
        if property == DELIVERY_STATUS_LOGGING {
            attributes.extend(delivery_status_attributes(value));
            continue;
        }
        attributes.insert(property.clone(), attribute_text(value));
    }
    attributes
}

/// The topic attributes a `DeliveryStatusLogging` list declares: one entry per member each
/// of its `LoggingConfig`s carries, under that config's protocol prefix
/// ([`LOGGING_PROTOCOL_PREFIXES`]).
///
/// A member the config does not declare is absent. `Protocol` is required on a
/// `LoggingConfig`, so an entry carrying none is a template error: its members go through
/// under their BARE names, which are not topic attribute names, so SNS refuses them — the
/// same way an unknown protocol spelling goes through as the template wrote it. Neither is
/// dropped, which would configure the topic differently from what the template declared
/// while the stack still completed.
pub(super) fn delivery_status_attributes(declared: &Value) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    for config in declared.as_array().map(Vec::as_slice).unwrap_or_default() {
        let prefix = config
            .get("Protocol")
            .and_then(Value::as_str)
            .map_or("", |protocol| {
                LOGGING_PROTOCOL_PREFIXES
                    .iter()
                    .find(|(declared, _)| declared.eq_ignore_ascii_case(protocol))
                    .map_or(protocol, |(_, prefix)| prefix)
            });
        for member in LOGGING_CONFIG_MEMBERS {
            if let Some(value) = config.get(*member) {
                attributes.insert(format!("{prefix}{member}"), attribute_text(value));
            }
        }
    }
    attributes
}

/// The `Subscribe` parameters each entry of a `Subscription` property declares: its
/// `Protocol` and its `Endpoint`, named identically on both sides. A member the entry does
/// not declare is absent from the call, so SNS's own validation is what refuses an
/// incomplete subscription.
pub(super) fn declared_subscriptions(properties: &Value) -> Vec<Vec<(String, String)>> {
    properties
        .get("Subscription")
        .and_then(Value::as_array)
        .map(|declared| {
            declared
                .iter()
                .map(|subscription| {
                    ["Protocol", "Endpoint"]
                        .iter()
                        .filter_map(|member| {
                            let value = subscription.get(*member)?;
                            Some(((*member).to_string(), attribute_text(value)))
                        })
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Amazon SNS: AWS::SNS::Subscription
// ---------------------------------------------------------------------------

/// The properties of an `AWS::SNS::Subscription` that are `Subscribe` ATTRIBUTES, named
/// identically on both sides: each is one entry of the `Attributes` map `Subscribe` and
/// `SetSubscriptionAttributes` take ("DeliveryPolicy", "FilterPolicy", "FilterPolicyScope",
/// "RawMessageDelivery", "RedrivePolicy", "ReplayPolicy", "SubscriptionRoleArn" are the
/// names both the resource type's properties and that map use).
///
/// Every other property of the type is a member of the `Subscribe` call itself
/// ([`SUBSCRIPTION_IDENTITY_PROPERTIES`]), so nothing is dropped.
pub(super) const SUBSCRIPTION_ATTRIBUTE_PROPERTIES: &[&str] = &[
    "DeliveryPolicy",
    "FilterPolicy",
    "FilterPolicyScope",
    "RawMessageDelivery",
    "RedrivePolicy",
    "ReplayPolicy",
    "SubscriptionRoleArn",
];

/// The properties that make a subscription what it IS — the topic it extends, the protocol
/// it is delivered over, the endpoint it reaches and the Region its topic lives in. Amazon
/// SNS has no call that moves an existing subscription to another of them, so a declaration
/// that changes one describes a different subscription.
pub(super) const SUBSCRIPTION_IDENTITY_PROPERTIES: &[&str] =
    &["TopicArn", "Protocol", "Endpoint", "Region"];

/// What a subscription attribute goes back to when the declaration stops carrying it: the
/// value Amazon SNS documents as that attribute's absent state.
///
/// A policy-valued attribute is REMOVED by an empty value ("you can remove a filter policy
/// by setting the FilterPolicy attribute to an empty value"), raw delivery goes back to the
/// envelope SNS wraps a notification in by default, and the filter scope back to the
/// message attributes ("MessageAttributes (default)"). `SubscriptionRoleArn` carries no
/// documented absent value, so a declaration that drops it leaves the role as it is rather
/// than writing a value SNS does not define.
pub(super) const SUBSCRIPTION_ATTRIBUTE_CLEARED: &[(&str, &str)] = &[
    ("DeliveryPolicy", ""),
    ("FilterPolicy", ""),
    ("RedrivePolicy", ""),
    ("ReplayPolicy", ""),
    ("RawMessageDelivery", "false"),
    ("FilterPolicyScope", "MessageAttributes"),
];

/// The Amazon SNS call that writes one attribute of a subscription that already exists.
pub(super) const SNS_SET_SUBSCRIPTION_ATTRIBUTES: &str = "SetSubscriptionAttributes";

/// The Region the topic a subscription extends lives in: the `Region` property — "for
/// cross-region subscriptions, the region in which the topic resides" — or the stack's own
/// Region when the declaration names none.
pub(super) fn subscription_region<'a>(properties: &'a Value, region: &'a str) -> &'a str {
    properties
        .get("Region")
        .and_then(Value::as_str)
        .filter(|named| !named.is_empty())
        .unwrap_or(region)
}

/// The `Attributes` map an `AWS::SNS::Subscription`'s declared properties are, as
/// `Subscribe` takes them.
///
/// An attribute value is a STRING on the wire, so a boolean or a policy document the
/// template wrote as such is rendered as the text it is ([`attribute_text`]), exactly as a
/// topic's attributes are. A property SNS does not know is not filtered out: the owning
/// service's own validation is what judges an attribute, and its refusal leaves the stack
/// `CREATE_FAILED` rather than a subscription configured differently from the template.
pub(super) fn subscription_attributes(properties: &Value) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    for property in SUBSCRIPTION_ATTRIBUTE_PROPERTIES {
        if let Some(value) = properties.get(*property) {
            attributes.insert((*property).to_string(), attribute_text(value));
        }
    }
    attributes
}

/// Subscribe the endpoint an `AWS::SNS::Subscription` declares to the topic it names.
///
/// A subscription is a resource of Amazon SNS's own, created by the one call a client makes
/// — `Subscribe`, with the protocol and endpoint the declaration names and the attribute map
/// [`subscription_attributes`] reads off its remaining properties — so the subscriber the
/// stack declares really is a subscriber of that topic: a later `Publish` delivers to it and
/// `ListSubscriptionsByTopic` reports it, like any subscription a client made itself.
///
/// The ARN the call answers with is this resource's physical id ("Ref returns the
/// subscription ARN"), which is what `Unsubscribe` takes when the stack is deleted. It is
/// asked for unconditionally (`ReturnSubscriptionArn`), because a subscription whose
/// subscriber has still to confirm answers with the `pending confirmation` sentinel in its
/// place, and a stack cannot address a resource by a sentinel.
pub(super) async fn subscribe(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let properties = &resource.properties;
    let mut parameters = Vec::new();
    for member in ["TopicArn", "Protocol", "Endpoint"] {
        if let Some(value) = properties.get(member) {
            parameters.push((member.to_string(), attribute_text(value)));
        }
    }
    parameters.push(("ReturnSubscriptionArn".to_string(), "true".to_string()));
    for (index, (name, value)) in subscription_attributes(properties).iter().enumerate() {
        let entry = index + 1;
        parameters.push((format!("Attributes.entry.{entry}.key"), name.clone()));
        parameters.push((format!("Attributes.entry.{entry}.value"), value.clone()));
    }
    let answered = call_query(
        ctx,
        SNS_SIGNING_NAME,
        subscription_region(properties, region),
        SNS_API_VERSION,
        "Subscribe",
        &parameters,
    )
    .await?;
    let arn = xml_element(&answered, "SubscriptionArn")
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon SNS answered Subscribe without a SubscriptionArn".to_string(),
        })?;
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes: BTreeMap::new(),
    })
}

/// Apply a CHANGED `AWS::SNS::Subscription` declaration to the subscription that already
/// exists: every attribute whose declared value moved is written through the one call that
/// changes an attribute of a live subscription, and one the declaration stopped carrying
/// goes back to its documented absent value ([`SUBSCRIPTION_ATTRIBUTE_CLEARED`]).
///
/// `Ok(None)` for a change to what the subscription IS
/// ([`SUBSCRIPTION_IDENTITY_PROPERTIES`]): SNS has no call that moves a subscription to
/// another topic, protocol or endpoint, so nothing is asked of it and the deployment reports
/// the change as not applied rather than as a success that dropped it.
pub(super) async fn update_subscription(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    for property in SUBSCRIPTION_IDENTITY_PROPERTIES {
        if declared.get(*property) != resource.properties.get(*property) {
            return Ok(None);
        }
    }
    let region = subscription_region(&resource.properties, region);
    let held = subscription_attributes(&resource.properties);
    let wanted = subscription_attributes(declared);
    for (name, value) in &wanted {
        if held.get(name) != Some(value) {
            set_subscription_attribute(
                ctx,
                region,
                &resource.physical_resource_id,
                name,
                value,
            )
            .await?;
        }
    }
    for name in held.keys() {
        if wanted.contains_key(name) {
            continue;
        }
        let Some((_, absent)) = SUBSCRIPTION_ATTRIBUTE_CLEARED
            .iter()
            .find(|(attribute, _)| attribute == name)
        else {
            continue;
        };
        set_subscription_attribute(ctx, region, &resource.physical_resource_id, name, absent)
            .await?;
    }
    Ok(Some(Created {
        physical_resource_id: None,
        attributes: BTreeMap::new(),
    }))
}

/// One `SetSubscriptionAttributes` writing `name` on the subscription `subscription_arn`
/// names.
pub(super) async fn set_subscription_attribute(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subscription_arn: &str,
    name: &str,
    value: &str,
) -> Result<(), Refusal> {
    let parameters = vec![
        ("SubscriptionArn".to_string(), subscription_arn.to_string()),
        ("AttributeName".to_string(), name.to_string()),
        ("AttributeValue".to_string(), value.to_string()),
    ];
    call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        SNS_SET_SUBSCRIPTION_ATTRIBUTES,
        &parameters,
    )
    .await
    .map(|_| ())
}

/// Delete the subscription a stack created, by the ARN the create reported.
///
/// A subscription SNS no longer holds is already in the state the delete asks for — its
/// topic may have been deleted with it, which takes its subscriptions along — so SNS's own
/// not-found code is a success here, exactly as it is for the topic.
pub(super) async fn unsubscribe(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "SubscriptionArn".to_string(),
        resource.physical_resource_id.clone(),
    )];
    match call_query(
        ctx,
        SNS_SIGNING_NAME,
        subscription_region(&resource.properties, region),
        SNS_API_VERSION,
        "Unsubscribe",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The subscription's configuration as Amazon SNS reports it, for drift detection.
///
/// A subscription SNS no longer has (`NotFound`) was deleted. Its configuration IS its
/// attribute map, which carries both the members that make it what it is (`TopicArn`,
/// `Protocol`, `Endpoint`) and the attributes a declaration may set — named identically to
/// this type's properties — so a subscription whose filter policy was changed outside the
/// stack is MODIFIED.
pub(super) async fn describe_subscription(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "SubscriptionArn".to_string(),
        resource.physical_resource_id.clone(),
    )];
    match call_query(
        ctx,
        SNS_SIGNING_NAME,
        subscription_region(&resource.properties, region),
        SNS_API_VERSION,
        "GetSubscriptionAttributes",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_entry_map(&answered, "Attributes")))),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon SageMaker: AWS::SageMaker::ModelPackageGroup
// ---------------------------------------------------------------------------
/// SageMaker's signing name and `X-Amz-Target` prefix, from its model (`sigv4Name:
/// sagemaker`, service shape `SageMaker`), which speaks `awsJson1_1`.
pub(super) const SAGEMAKER_SIGNING_NAME: &str = "sagemaker";
pub(super) const SAGEMAKER_TARGET_PREFIX: &str = "SageMaker";

/// The property `AWS::SageMaker::ModelPackageGroup` carries the model group's resource
/// policy under.
///
/// It is the ONE property of that type which is not a `CreateModelPackageGroup` member:
/// SageMaker attaches a model group's resource policy through its own
/// `PutModelPackageGroupPolicy`, so the property is taken out of the create request and
/// applied with that call once the group exists — the same two calls a client makes.
pub(super) const MODEL_PACKAGE_GROUP_POLICY: &str = "ModelPackageGroupPolicy";

/// Create the model group an `AWS::SageMaker::ModelPackageGroup` declares, with the
/// resource policy it declares for it.
///
/// `ModelPackageGroupName`, `ModelPackageGroupDescription` and `Tags` are named identically
/// on the type and on `CreateModelPackageGroup`, so the resolved properties ARE the
/// request, with the one member CloudFormation decides (the group's name, which is this
/// resource's physical id) set on the way out, and the `Tags` list carrying the tags
/// CloudFormation propagates beside the declared ones ([`tags_of`]) — `CreateModelPackageGroup$Tags`
/// is the same `Tag{Key, Value}` list the type's property is. [`MODEL_PACKAGE_GROUP_POLICY`]
/// is the exception, and is attached afterwards through the operation that owns it; a group
/// whose policy SageMaker refuses is not left behind half-configured — it is deleted again,
/// and the refusal is what the deployment reports, with a cleanup that itself failed named
/// in it rather than swallowed.
pub(super) async fn create_model_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let policy = request.remove(MODEL_PACKAGE_GROUP_POLICY);
    let name = resource.physical_resource_id.clone();
    request.insert(
        "ModelPackageGroupName".to_string(),
        Value::String(name.clone()),
    );
    let tags = tags_of(&resource.properties, propagated);
    if tags.is_empty() {
        request.remove("Tags");
    } else {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.CreateModelPackageGroup"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("ModelPackageGroupArn").and_then(Value::as_str) {
        // The group's real ARN, which is the ARN its `Fn::GetAtt` reports.
        attributes.insert("Arn".to_string(), arn.to_string());
        attributes.insert("ModelPackageGroupArn".to_string(), arn.to_string());
    }

    let Some(policy) = policy.filter(|policy| !policy.is_null()) else {
        return Ok(Created::attributes(attributes));
    };
    // The type's property is a `Json` document, which a template may spell as the document
    // itself or as the text of one; `PutModelPackageGroupPolicy$ResourcePolicy` is modelled
    // a string either way, so the one JSON-to-text rendering this module has is what
    // renders it: text stays itself, a document becomes its own compact JSON.
    let document = attribute_text(&policy);
    let attached = call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.PutModelPackageGroupPolicy"),
        &serde_json::json!({
            "ModelPackageGroupName": name,
            "ResourcePolicy": document,
        }),
    )
    .await;
    let Err(refusal) = attached else {
        return Ok(Created::attributes(attributes));
    };
    // The group exists but carries none of the policy the template declared, and the
    // deployment will not record it as created: delete it rather than leave a group behind
    // that no stack would ever clean up. A cleanup SageMaker refuses leaves one there, so
    // that refusal is reported alongside the one that caused it.
    match delete_model_package_group(ctx, region, &name).await {
        Ok(()) => Err(refusal),
        Err(cleanup) => Err(Refusal {
            code: refusal.code,
            message: format!(
                "{}; the model package group could not be deleted again: {cleanup}",
                refusal.message
            ),
        }),
    }
}

/// The error code SageMaker answers for a model group it does not have: it validates the
/// group's existence before the call rather than modelling a not-found error, so this is
/// what `DescribeModelPackageGroup` answers for a group that is absent.
pub(super) const SAGEMAKER_VALIDATION_EXCEPTION: &str = "ValidationException";

/// Delete the model group a stack created. SageMaker releases a group's resource policy
/// with the group, so the one call removes both.
///
/// A group SageMaker no longer has is already in the state the delete asks for. This
/// operation models no not-found error — the service validates the group's existence and
/// answers `ValidationException`, which is also what it answers for a group it refuses to
/// delete (one that still contains model package versions) — so "already gone" is decided
/// by READING the group back rather than by matching the refusal's text. Only the read's
/// OWN `ValidationException` means absence: any other answer (a transport fault, an
/// internal failure) leaves the delete's refusal standing rather than reporting a deletion
/// nothing carried out.
pub(super) async fn delete_model_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "ModelPackageGroupName": physical_resource_id });
    let refusal = match call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.DeleteModelPackageGroup"),
        &request,
    )
    .await
    {
        Ok(_) => return Ok(()),
        Err(refusal) => refusal,
    };
    let described = call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.DescribeModelPackageGroup"),
        &request,
    )
    .await;
    if matches!(&described, Err(absence) if absence.code == SAGEMAKER_VALIDATION_EXCEPTION) {
        // SageMaker does not have the group, which is the state the delete asked for.
        return Ok(());
    }
    Err(refusal)
}

/// The model group's configuration as SageMaker reports it, for drift detection.
///
/// A group SageMaker no longer holds answers `ValidationException` (it validates the
/// group's existence rather than modelling a not-found error), which is the `DELETED`
/// drift status. The group's own resource policy is not part of that answer, so it is read
/// through the operation that owns it and reported under the property the template declares
/// it as; a group carrying none simply has no such property, and a property the owning
/// service does not report is not evidence of drift.
pub(super) async fn describe_model_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "ModelPackageGroupName": physical_resource_id });
    let described = match call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.DescribeModelPackageGroup"),
        &request,
    )
    .await
    {
        Ok(described) => described,
        Err(refusal) if refusal.code == SAGEMAKER_VALIDATION_EXCEPTION => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(mut configuration) = described.as_object().cloned() else {
        return Ok(Some(described));
    };
    // The policy read's own absence refusal means the group carries no policy, which is a
    // configuration without that property. Any OTHER refusal is a read that did not
    // happen, and is propagated rather than reported as a property the service does not
    // have.
    match call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.GetModelPackageGroupPolicy"),
        &request,
    )
    .await
    {
        Ok(policy) => {
            if let Some(document) = policy.get("ResourcePolicy") {
                configuration.insert(MODEL_PACKAGE_GROUP_POLICY.to_string(), document.clone());
            }
        }
        Err(refusal) if refusal.code == SAGEMAKER_VALIDATION_EXCEPTION => {}
        Err(refusal) => return Err(refusal),
    }
    Ok(Some(Value::Object(configuration)))
}

// ---------------------------------------------------------------------------
// AWS Fault Injection Service: AWS::FIS::ExperimentTemplate
// ---------------------------------------------------------------------------

/// AWS FIS's signing name, from its model (`sigv4Name: fis`). FIS speaks `restJson1`, so
/// its operations are bound to a METHOD and a URI rather than to an `X-Amz-Target` header:
/// `CreateExperimentTemplate` is `POST /experimentTemplates` and
/// `DeleteExperimentTemplate` is `DELETE /experimentTemplates/{id}`
/// (`smithy_query(query="operation", operation="CreateExperimentTemplate,\
/// DeleteExperimentTemplate")` on that service's model).
pub(super) const FIS_SIGNING_NAME: &str = "fis";

/// The URI `CreateExperimentTemplate` is bound to.
pub(super) const FIS_EXPERIMENT_TEMPLATES_URI: &str = "/experimentTemplates";

/// The error code FIS answers for an experiment template it does not hold
/// (`ResourceNotFoundException`, a modelled error of `DeleteExperimentTemplate`).
pub(super) const FIS_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `AWS::FIS::ExperimentTemplate` properties `CreateExperimentTemplate` takes whose
/// value goes through unchanged under its lower-camel member name, as (property, member)
/// pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against the
/// `CreateExperimentTemplateRequest` shape. The FIS model spells its members in lower
/// camel case and renames nothing, so the only difference is the first character's case;
/// `StopConditions`, `Targets`, `Actions`, `LogConfiguration` and `ExperimentOptions` are
/// NOT here because their own members have to be re-cased one structure at a time (a map
/// of targets or of actions is keyed by names the TEMPLATE chose, which must not be
/// re-cased), and `Tags` is not because the type spells it as a map of tag keys the
/// template chose.
pub(super) const FIS_SCALAR_MEMBERS: &[(&str, &str)] =
    &[("Description", "description"), ("RoleArn", "roleArn")];

/// The members of one `ExperimentTemplateStopCondition` the type declares.
pub(super) const FIS_STOP_CONDITION_MEMBERS: &[(&str, &str)] = &[("Source", "source"), ("Value", "value")];

/// The members of one `ExperimentTemplateTarget` the type declares. `ResourceTags` and
/// `Parameters` are maps keyed by DATA (tag keys, and the target parameter names the
/// resource type publishes), so their values go through as the template wrote them.
pub(super) const FIS_TARGET_MEMBERS: &[(&str, &str)] = &[
    ("ResourceType", "resourceType"),
    ("SelectionMode", "selectionMode"),
    ("ResourceArns", "resourceArns"),
    ("ResourceTags", "resourceTags"),
    ("Parameters", "parameters"),
];

/// The members of one `ExperimentTemplateTargetFilter`, which a target's `Filters` list
/// carries.
pub(super) const FIS_FILTER_MEMBERS: &[(&str, &str)] = &[("Path", "path"), ("Values", "values")];

/// The members of one `ExperimentTemplateAction` the type declares. `Parameters` is keyed
/// by the action's published parameter names and `Targets` by the action's published target
/// names, so both go through as the template wrote them.
pub(super) const FIS_ACTION_MEMBERS: &[(&str, &str)] = &[
    ("ActionId", "actionId"),
    ("Description", "description"),
    ("Parameters", "parameters"),
    ("Targets", "targets"),
    ("StartAfter", "startAfter"),
];

/// The members of the type's `LogConfiguration` property, and of the two configurations it
/// nests.
pub(super) const FIS_LOG_CONFIGURATION_MEMBERS: &[(&str, &str)] = &[("LogSchemaVersion", "logSchemaVersion")];
pub(super) const FIS_CLOUDWATCH_LOGS_MEMBERS: &[(&str, &str)] = &[("LogGroupArn", "logGroupArn")];
pub(super) const FIS_S3_LOG_MEMBERS: &[(&str, &str)] = &[("BucketName", "bucketName"), ("Prefix", "prefix")];

/// The members of the type's `ExperimentOptions` property.
pub(super) const FIS_EXPERIMENT_OPTIONS_MEMBERS: &[(&str, &str)] = &[
    ("AccountTargeting", "accountTargeting"),
    ("EmptyTargetResolutionMode", "emptyTargetResolutionMode"),
];

/// The one property of the type this provisioner does not map onto the create's request.
pub(super) const FIS_REPORT_CONFIGURATION: &str = "ExperimentReportConfiguration";

/// Create the experiment template an `AWS::FIS::ExperimentTemplate` declares, so a client
/// that reads the id out of the stack's outputs can call FIS with it
/// (`GetExperimentTemplate`, `StartExperiment`) rather than being handed an id FIS never
/// minted.
///
/// FIS ASSIGNS the id — `Ref` of this type returns it, and the type has no name property —
/// so the id this call answers with becomes the resource's physical id, replacing the
/// stand-in the naming rules computed, and is reported back as the `Id` attribute
/// `Fn::GetAtt` reads.
///
/// `clientToken` is the create's `@idempotencyToken`, required by the model: it is the
/// computed physical id, which is a function of the stack and the resource's logical id, so
/// the same declaration of the same stack asks for the same template rather than a second
/// one.
pub(super) async fn create_experiment_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    if properties.contains_key(FIS_REPORT_CONFIGURATION) {
        // The property is not mapped onto `CreateExperimentTemplate`'s own
        // `experimentReportConfiguration`, so creating the template would create one
        // configured differently from the declaration. Refused rather than silently
        // dropped; mapping it is its own fix.
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not implement the {FIS_REPORT_CONFIGURATION} property of \
                 {}",
                resource.resource_type
            ),
        });
    }
    let mut request = Map::new();
    request.insert(
        "clientToken".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    copy_members(&properties, FIS_SCALAR_MEMBERS, &mut request);
    if let Some(Value::Array(conditions)) = properties.get("StopConditions") {
        let conditions: Vec<Value> = conditions
            .iter()
            .map(|condition| mapped_members(condition, FIS_STOP_CONDITION_MEMBERS))
            .collect();
        request.insert("stopConditions".to_string(), Value::Array(conditions));
    }
    if let Some(Value::Object(targets)) = properties.get("Targets") {
        let targets: Map<String, Value> = targets
            .iter()
            .map(|(name, target)| (name.clone(), experiment_target(target)))
            .collect();
        request.insert("targets".to_string(), Value::Object(targets));
    }
    if let Some(Value::Object(actions)) = properties.get("Actions") {
        let actions: Map<String, Value> = actions
            .iter()
            .map(|(name, action)| (name.clone(), mapped_members(action, FIS_ACTION_MEMBERS)))
            .collect();
        request.insert("actions".to_string(), Value::Object(actions));
    }
    if let Some(configuration) = properties.get("LogConfiguration") {
        request.insert(
            "logConfiguration".to_string(),
            log_configuration(configuration),
        );
    }
    if let Some(options) = properties.get("ExperimentOptions") {
        request.insert(
            "experimentOptions".to_string(),
            mapped_members(options, FIS_EXPERIMENT_OPTIONS_MEMBERS),
        );
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        FIS_SIGNING_NAME,
        region,
        "POST",
        FIS_EXPERIMENT_TEMPLATES_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(id) = reported
        .get("experimentTemplate")
        .and_then(|template| template.get("id"))
        .and_then(Value::as_str)
    else {
        // FIS answered a success carrying no id, so there is nothing a client could call
        // it with: reported as a refusal rather than recorded as a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateExperimentTemplate answered no experiment template id".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), id.to_string());
    Ok(Created {
        physical_resource_id: Some(id.to_string()),
        attributes,
    })
}

/// Delete the experiment template a stack created, by the id FIS minted for it — this
/// resource's physical id. A template FIS no longer holds
/// (`ResourceNotFoundException`) is already in the state the delete asks for.
pub(super) async fn delete_experiment_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        FIS_SIGNING_NAME,
        region,
        "DELETE",
        &format!("{FIS_EXPERIMENT_TEMPLATES_URI}/{physical_resource_id}"),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == FIS_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The experiment template's configuration as FIS reports it, for drift detection:
/// `GetExperimentTemplate`'s own record, read by the id FIS minted.
///
/// A template FIS no longer holds (`ResourceNotFoundException`) was deleted. The record's
/// members are FIS's own (lower camel), so only a declared property spelled identically on
/// both sides takes part in the comparison — the position the Amazon ECS arm above is in
/// too, and what [`super::drift`] documents: a name mismatch reports the resource as
/// existing rather than as modified.
pub(super) async fn describe_experiment_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        FIS_SIGNING_NAME,
        region,
        "GET",
        &format!("{FIS_EXPERIMENT_TEMPLATES_URI}/{physical_resource_id}"),
        None,
    )
    .await
    {
        Ok(answer) => Ok(answer.get("experimentTemplate").cloned()),
        Err(refusal) if refusal.code == FIS_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// One `ExperimentTemplateTarget` as `CreateExperimentTemplate` takes it: its own members
/// re-cased, and its `Filters` list — whose entries are modelled structures of their own —
/// mapped entry by entry.
pub(super) fn experiment_target(target: &Value) -> Value {
    let mut mapped = mapped_members(target, FIS_TARGET_MEMBERS);
    if let Some(Value::Array(filters)) = target.get("Filters") {
        let filters: Vec<Value> = filters
            .iter()
            .map(|filter| mapped_members(filter, FIS_FILTER_MEMBERS))
            .collect();
        if let Value::Object(members) = &mut mapped {
            members.insert("filters".to_string(), Value::Array(filters));
        }
    }
    mapped
}

/// The type's `LogConfiguration` property as `CreateExperimentTemplate` takes it: its own
/// member plus the two nested configurations, each mapped member by member.
pub(super) fn log_configuration(configuration: &Value) -> Value {
    let mut mapped = mapped_members(configuration, FIS_LOG_CONFIGURATION_MEMBERS);
    let Value::Object(members) = &mut mapped else {
        return mapped;
    };
    if let Some(logs) = configuration.get("CloudWatchLogsConfiguration") {
        members.insert(
            "cloudWatchLogsConfiguration".to_string(),
            mapped_members(logs, FIS_CLOUDWATCH_LOGS_MEMBERS),
        );
    }
    if let Some(s3) = configuration.get("S3Configuration") {
        members.insert(
            "s3Configuration".to_string(),
            mapped_members(s3, FIS_S3_LOG_MEMBERS),
        );
    }
    mapped
}

/// One declared structure with the members `pairs` names mapped onto the member names the
/// owning service's model spells, and every value passed through as the template wrote it.
///
/// A property `pairs` does not name is one the create has no member for, so it is not
/// sent: the request carries what the declaration said and nothing invented.
pub(super) fn mapped_members(declared: &Value, pairs: &[(&str, &str)]) -> Value {
    let mut mapped = Map::new();
    if let Some(members) = declared.as_object() {
        copy_members(members, pairs, &mut mapped);
    }
    Value::Object(mapped)
}

/// Copy the members `pairs` names out of `declared` into `request`, under the member name
/// each pair gives.
pub(super) fn copy_members(
    declared: &Map<String, Value>,
    pairs: &[(&str, &str)],
    request: &mut Map<String, Value>,
) {
    for (property, member) in pairs {
        if let Some(value) = declared.get(*property) {
            request.insert((*member).to_string(), value.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// AWS WAF Classic, regional: AWS::WAFRegional::IPSet
// ---------------------------------------------------------------------------

/// AWS WAF Classic Regional's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: waf-regional`, service shape `AWSWAF_Regional_20161128`), which speaks
/// `awsJson1_1`.
pub(super) const WAF_REGIONAL_SIGNING_NAME: &str = "waf-regional";
pub(super) const WAF_REGIONAL_TARGET_PREFIX: &str = "AWSWAF_Regional_20161128";

/// The error code AWS WAF Classic answers for an object it does not hold
/// (`WAFNonexistentItemException`: "the referenced object doesn't exist"), and the one
/// `UpdateIPSet` answers when the IPSet the updates are for is not there at all
/// (`WAFNonexistentContainerException`: "you tried to add an IP address to or delete an IP
/// address from an IPSet that doesn't exist").
pub(super) const WAF_NONEXISTENT_ITEM: &str = "WAFNonexistentItemException";
pub(super) const WAF_NONEXISTENT_CONTAINER: &str = "WAFNonexistentContainerException";

/// The two `ChangeAction` values an `UpdateIPSet` update carries: "whether to insert or
/// delete the IP address".
pub(super) const WAF_INSERT: &str = "INSERT";
pub(super) const WAF_DELETE: &str = "DELETE";

/// Create the IPSet an `AWS::WAFRegional::IPSet` declares, and answer the `IPSetId` AWS WAF
/// Classic minted as the resource's physical id — "Ref returns the IPSet ID", which is a
/// value only that service produces.
///
/// The type's properties are `Name` and `IPSetDescriptors`, and AWS WAF Classic takes them
/// through the two calls its own documentation sequences: "Submit a CreateIPSet request …
/// Submit an UpdateIPSet request to specify the IP addresses that you want AWS WAF to watch
/// for". So the create carries the name, and each declared descriptor is INSERTed by one
/// update. Every mutating call carries its own change token ("Use GetChangeToken to get the
/// change token that you provide in the ChangeToken parameter"), so a resource whose
/// descriptors the template declares is created with them rather than with the name alone.
pub(super) async fn create_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let request = serde_json::json!({
        "Name": declared_name(resource),
        "ChangeToken": waf_regional_change_token(ctx, region).await?,
    });
    let reported = call_waf_regional(ctx, region, "CreateIPSet", &request).await?;
    let Some(ip_set_id) = reported
        .pointer("/IPSet/IPSetId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
    else {
        // The create answered success and identified nothing, so there is no id to address
        // the IPSet by: not something this stack can report as a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateIPSet answered no IPSet.IPSetId".to_string(),
        });
    };
    let descriptors = declared_ip_set_descriptors(&resource.properties);
    if !descriptors.is_empty() {
        let updates: Vec<Value> = descriptors
            .into_iter()
            .map(|descriptor| {
                serde_json::json!({ "Action": WAF_INSERT, "IPSetDescriptor": descriptor })
            })
            .collect();
        if let Err(refusal) = update_ip_set(ctx, region, &ip_set_id, updates).await {
            // The resource the template declares is the IPSet WITH its addresses, and this
            // create could not make it. The empty IPSet the first call left is deleted
            // again, so a create the stack reports as failed leaves nothing behind in the
            // owning service; the refusal reported is the one that failed.
            let _ = delete_ip_set(ctx, region, &ip_set_id).await;
            return Err(refusal);
        }
    }
    Ok(Created {
        physical_resource_id: Some(ip_set_id),
        attributes: BTreeMap::new(),
    })
}

/// Delete the IPSet a stack created.
///
/// "You can't delete an IPSet if it still includes any IP addresses", so the addresses go
/// first, which is the sequence AWS WAF Classic documents for permanently deleting one:
/// "Update the IPSet to remove IP address ranges, if any. Submit a DeleteIPSet request".
/// The IPSet is read back, every descriptor it still holds is DELETEd by one update, and
/// the IPSet itself is then deleted.
///
/// An IPSet AWS WAF Classic no longer holds is already in the state the delete asks for,
/// judged on its own not-found CODES (`WAFNonexistentItemException` from the read and the
/// delete, `WAFNonexistentContainerException` from the update that empties it) and never on
/// a message, which carries the request's own id. One a Rule still references is NOT
/// deleted (`WAFReferencedItemException`), and that refusal is what the stack reports.
pub(super) async fn delete_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let Some(ip_set) = describe_ip_set(ctx, region, physical_resource_id).await? else {
        return Ok(());
    };
    let descriptors = ip_set
        .get("IPSetDescriptors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !descriptors.is_empty() {
        let updates: Vec<Value> = descriptors
            .into_iter()
            .map(|descriptor| {
                serde_json::json!({ "Action": WAF_DELETE, "IPSetDescriptor": descriptor })
            })
            .collect();
        match update_ip_set(ctx, region, physical_resource_id, updates).await {
            Ok(()) => {}
            Err(refusal) if refusal.code == WAF_NONEXISTENT_CONTAINER => return Ok(()),
            Err(refusal) => return Err(refusal),
        }
    }
    let request = serde_json::json!({
        "IPSetId": physical_resource_id,
        "ChangeToken": waf_regional_change_token(ctx, region).await?,
    });
    match call_waf_regional(ctx, region, "DeleteIPSet", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == WAF_NONEXISTENT_ITEM => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The IPSet as AWS WAF Classic reports it, for drift detection and for the delete that has
/// to empty it first. An IPSet it no longer holds (`WAFNonexistentItemException`) was
/// deleted.
///
/// The `IPSet` the read answers carries `Name` and `IPSetDescriptors` under exactly the
/// names the type's properties have, so it is the actual configuration the declared one is
/// compared against.
pub(super) async fn describe_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "IPSetId": physical_resource_id });
    match call_waf_regional(ctx, region, "GetIPSet", &request).await {
        Ok(answer) => Ok(Some(reported_member(&answer, "IPSet"))),
        Err(refusal) if refusal.code == WAF_NONEXISTENT_ITEM => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Apply one batch of `Updates` to an IPSet, under a change token of its own.
pub(super) async fn update_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    ip_set_id: &str,
    updates: Vec<Value>,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "IPSetId": ip_set_id,
        "ChangeToken": waf_regional_change_token(ctx, region).await?,
        "Updates": updates,
    });
    call_waf_regional(ctx, region, "UpdateIPSet", &request)
        .await
        .map(|_| ())
}

/// The `IPSetDescriptors` an `AWS::WAFRegional::IPSet` declares, as `UpdateIPSet` takes
/// them.
///
/// An `IPSetDescriptor` is `{Type, Value}` on both sides — the property's members are the
/// API shape's own — so each entry goes through as the template wrote it. A declaration
/// AWS WAF Classic will not take is not filtered out here: that service's own validation is
/// what judges a descriptor, and its refusal leaves the stack `CREATE_FAILED` rather than
/// an IPSet configured differently from the template.
pub(super) fn declared_ip_set_descriptors(properties: &Value) -> Vec<Value> {
    properties
        .get("IPSetDescriptors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The NAME a resource record declares, for a type whose PHYSICAL id is not that name: the
/// identifier the owning service minted, or the resource's ARN where that is what the type's
/// `Ref` returns. It is the `Name` the record's attribute carries, which is the name the
/// template asked for or the `<stack>-<logical id>-<suffix>` name CloudFormation generated
/// for it — never the physical id.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to the
/// physical id, which before the create IS that computed name. Shared by every arm in that
/// position (an AWS WAF Classic regional IPSet, an Amazon Managed Grafana workspace), so
/// the create of one cannot read the declared name differently from the create of another.
pub(crate) fn declared_name(resource: &StoredResource) -> String {
    resource
        .attributes
        .get("Name")
        .cloned()
        .unwrap_or_else(|| resource.physical_resource_id.clone())
}

/// A change token for one mutating AWS WAF Classic call. "Each create, update, or delete
/// request must use a unique change token", so one is fetched per call rather than reused.
pub(super) async fn waf_regional_change_token(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Result<String, Refusal> {
    let answer = call_waf_regional(ctx, region, "GetChangeToken", &serde_json::json!({})).await?;
    answer
        .get("ChangeToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "GetChangeToken answered no ChangeToken".to_string(),
        })
}

/// Call one AWS WAF Classic Regional operation on the awsJson1_1 wire its model declares.
pub(super) async fn call_waf_regional(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        WAF_REGIONAL_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{WAF_REGIONAL_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS WAF (WAFv2): AWS::WAFv2::WebACL, ::RuleGroup, ::IPSet, ::RegexPatternSet
// ---------------------------------------------------------------------------

/// WAF's signing name and `X-Amz-Target` prefix, from the WAFv2 model (`sigv4Name: wafv2`,
/// service shape `AWSWAF_20190729`), which speaks `awsJson1_1`.
pub(super) const WAFV2_SIGNING_NAME: &str = "wafv2";
pub(super) const WAFV2_TARGET_PREFIX: &str = "AWSWAF_20190729";

/// The code WAF answers for an entity it does not hold (`WAFNonexistentItemException`: "WAF
/// couldn't perform the operation because your resource doesn't exist").
pub(super) const WAFV2_NONEXISTENT_ITEM: &str = "WAFNonexistentItemException";

/// The member every WAFv2 create answers the created entity's high-level information under:
/// its name, the `Id` WAF assigned and the `ARN` WAF built, "that you provide to operations
/// like `AssociateWebACL`" (`WebACLSummary`, and the same shape per entity kind).
pub(super) const WAFV2_SUMMARY: &str = "Summary";

/// The property a WAFv2 entity declares the namespace it lives in under, which every call
/// addressing one carries: "specifies whether this is for a global resource type, such as a
/// Amazon CloudFront distribution" — `CLOUDFRONT` or `REGIONAL` (`Scope`).
pub(super) const WAFV2_SCOPE: &str = "Scope";

/// The statement whose search value is a BLOB on the wire, and the two spellings a template
/// declares that value in ([`wafv2_search_string_encoded`]).
pub(super) const WAFV2_BYTE_MATCH_STATEMENT: &str = "ByteMatchStatement";
pub(super) const WAFV2_SEARCH_STRING: &str = "SearchString";
pub(super) const WAFV2_SEARCH_STRING_BASE64: &str = "SearchStringBase64";

/// The statements that name ANOTHER WAFv2 entity: "create an IPSet that specifies the
/// addresses you want to detect, then use the ARN of that set in this statement"
/// (`IPSetReferenceStatement`), and the same for a regex pattern set and a rule group.
///
/// Each takes the referenced entity's ARN under a REQUIRED member, and that member is the
/// one a rule tree spells differently on the two sides ([`wafv2_reference_arn`]).
pub(super) const WAFV2_REFERENCE_STATEMENTS: &[&str] = &[
    "IPSetReferenceStatement",
    "RegexPatternSetReferenceStatement",
    "RuleGroupReferenceStatement",
];

/// The two spellings of that member: `ARN` is what the API declares
/// (`IPSetReferenceStatement$ARN`, `RegexPatternSetReferenceStatement$ARN`,
/// `RuleGroupReferenceStatement$ARN`, each `@required`), and `Arn` is what the resource
/// types write it as.
pub(super) const WAFV2_REFERENCE_ARN: &str = "ARN";
pub(super) const WAFV2_TEMPLATE_REFERENCE_ARN: &str = "Arn";

/// Which side's spelling a WAFv2 document is in ([`wafv2_spelled`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Wafv2Spelling {
    /// The members as the API's own model declares them, which is what a call carries.
    Wire,
    /// The members as a resource type's properties write them, which is what a template
    /// declares and what a drift comparison reads.
    Template,
}

/// The calls one KIND of WAFv2 entity is created, read and deleted through.
///
/// The four types this emulator provisions differ only in these names: each `Create…` takes
/// the type's properties as its own members and answers a [`WAFV2_SUMMARY`], each `Get…`
/// answers the entity under its own member beside the `LockToken` that read marks its state
/// at, and each `Delete…` takes `Name`, `Scope`, `Id` and that `LockToken`. So one create,
/// read and delete serve all four, and adding a WAFv2 type is a table entry.
pub(super) struct Wafv2Calls {
    /// The operation that creates one.
    pub(super) create: &'static str,
    /// The operation that reads one back.
    pub(super) get: &'static str,
    /// The member `get` answers the entity under (`WebACL`, `IPSet`, …).
    pub(super) entity: &'static str,
    /// The operation that deletes one.
    pub(super) delete: &'static str,
}

pub(super) const WAFV2_WEB_ACL_CALLS: &Wafv2Calls = &Wafv2Calls {
    create: "CreateWebACL",
    get: "GetWebACL",
    entity: "WebACL",
    delete: "DeleteWebACL",
};

pub(super) const WAFV2_RULE_GROUP_CALLS: &Wafv2Calls = &Wafv2Calls {
    create: "CreateRuleGroup",
    get: "GetRuleGroup",
    entity: "RuleGroup",
    delete: "DeleteRuleGroup",
};

pub(super) const WAFV2_IP_SET_CALLS: &Wafv2Calls = &Wafv2Calls {
    create: "CreateIPSet",
    get: "GetIPSet",
    entity: "IPSet",
    delete: "DeleteIPSet",
};

pub(super) const WAFV2_REGEX_PATTERN_SET_CALLS: &Wafv2Calls = &Wafv2Calls {
    create: "CreateRegexPatternSet",
    get: "GetRegexPatternSet",
    entity: "RegexPatternSet",
    delete: "DeleteRegexPatternSet",
};

/// Create the WAFv2 entity a declaration of one of the four types describes, and answer the
/// identity WAF MINTED for it: the `Id` it assigned as the resource's physical id, and the
/// `ARN` it built as the resource's `Arn`/`<Type>Arn` attributes.
///
/// Those identifiers are WAF's to produce, not CloudFormation's to compute: a WAFv2 ARN is
/// `…:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`, which carries the
/// scope the declaration asked for and the entity id that service assigned, so it can be
/// decomposed into the triple the API addresses an entity by. That is why it is read off
/// the create's own answer and reported back here: what the stack publishes for the
/// resource is what a client can hand straight to `AssociateWebACL`, a logging
/// configuration or a `Get…`, instead of a value that identifies nothing.
pub(super) async fn create_wafv2_entity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
    calls: &'static Wafv2Calls,
) -> Result<Created, Refusal> {
    let request = wafv2_create_request(resource, propagated)?;
    let answered = call_wafv2(ctx, region, calls.create, &request).await?;
    let summary = reported_member(&answered, WAFV2_SUMMARY);
    let (Some(id), Some(arn)) = (
        summary.get("Id").and_then(scalar_text),
        summary.get("ARN").and_then(scalar_text),
    ) else {
        // The create answered success and identified nothing, so there is neither an id to
        // address the entity by nor an ARN to publish for it: not something this stack can
        // report as a created resource.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!("{} answered no Summary.Id or Summary.ARN", calls.create),
        });
    };
    Ok(Created {
        physical_resource_id: Some(id),
        attributes: super::template::arn_attributes(&resource.resource_type, &arn)
            .into_iter()
            .collect(),
    })
}

/// Delete the WAFv2 entity a stack created.
///
/// The delete carries the `LockToken` of the entity as it stands — "you provide the token to
/// operations like update and delete. WAF uses the token to ensure that no changes have been
/// made to the entity since you last retrieved it" — so the entity is read first and the
/// token that read answered is what the delete is made with. One WAF no longer holds is
/// already in the state the delete asks for, judged on its own not-found CODE and never on a
/// message. One a protected resource is still associated with, or another entity still
/// references, is NOT deleted (`WAFAssociatedItemException`), and that refusal is what the
/// stack reports.
pub(super) async fn delete_wafv2_entity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    calls: &'static Wafv2Calls,
) -> Result<(), Refusal> {
    let Some((_, lock_token)) = read_wafv2_entity(ctx, region, resource, calls).await? else {
        return Ok(());
    };
    let Some(lock_token) = lock_token else {
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!("{} answered no LockToken", calls.get),
        });
    };
    let mut request = wafv2_entity_key(resource);
    if let Some(members) = request.as_object_mut() {
        members.insert("LockToken".to_string(), Value::String(lock_token));
    }
    match call_wafv2(ctx, region, calls.delete, &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == WAFV2_NONEXISTENT_ITEM => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The entity as WAF reports it, for drift detection. One it no longer holds was deleted.
///
/// The document the read answers carries the entity's members under exactly the names the
/// type's properties have (`Description`, `Rules`, `VisibilityConfig`, `Addresses`), so it
/// is the actual configuration the declared one is compared against — with each member
/// whose SPELLING differs between the two sides reported in both of the spellings a template
/// writes it in ([`wafv2_spelled`]), so an entity nobody changed is not
/// reported as modified whichever of them its declaration used.
pub(super) async fn describe_wafv2_entity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    calls: &'static Wafv2Calls,
) -> Result<Option<Value>, Refusal> {
    let Some((entity, _)) = read_wafv2_entity(ctx, region, resource, calls).await? else {
        return Ok(None);
    };
    wafv2_spelled(&entity, 0, Wafv2Spelling::Template).map(Some)
}

/// One WAFv2 entity as its own `Get…` answers it, with the `LockToken` that read marked its
/// state at. `None` is WAF answering that it no longer holds the entity
/// ([`WAFV2_NONEXISTENT_ITEM`]).
pub(super) async fn read_wafv2_entity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    calls: &'static Wafv2Calls,
) -> Result<Option<(Value, Option<String>)>, Refusal> {
    let request = wafv2_entity_key(resource);
    match call_wafv2(ctx, region, calls.get, &request).await {
        Ok(answer) => {
            let lock_token = answer.get("LockToken").and_then(scalar_text);
            Ok(Some((reported_member(&answer, calls.entity), lock_token)))
        }
        Err(refusal) if refusal.code == WAFV2_NONEXISTENT_ITEM => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `Create…` request a WAFv2 declaration is.
///
/// These types' properties ARE their create's own members, named identically on both sides
/// (`Scope`, `DefaultAction`, `Rules`, `VisibilityConfig`, `Addresses`,
/// `RegularExpressionList`, `Capacity`, `CustomResponseBodies`), so the evaluated property
/// document goes through as the template wrote it, with the one member CloudFormation
/// decides (the name) set on the way out and the tag list replaced by the set the resource is
/// to carry ([`tags_of`]) — WAF's `TagList` is `length 1..`, so a resource with no tag at all
/// carries the member not at all rather than an empty list.
///
/// The members whose SPELLING differs — a byte-match statement's search value and the ARN a
/// reference statement names another entity by — are put in the API's spelling on the way
/// out ([`wafv2_spelled`]).
pub(super) fn wafv2_create_request(
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Value, Refusal> {
    let declared = wafv2_spelled(&resource.properties, 0, Wafv2Spelling::Wire)?;
    let mut request = declared.as_object().cloned().unwrap_or_default();
    request.insert("Name".to_string(), Value::String(declared_name(resource)));
    let tags = tags_of(&resource.properties, propagated);
    if tags.is_empty() {
        request.remove("Tags");
    } else {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    Ok(Value::Object(request))
}

/// The triple WAF addresses one entity by: the NAME the declaration asked for, the SCOPE it
/// declared and the `Id` that service assigned, which is this resource's physical id.
///
/// A declaration with no `Scope` carries none, and WAF's own validation of its required
/// member is what refuses the call — this module never stands a value in for one the
/// template did not declare.
pub(super) fn wafv2_entity_key(resource: &StoredResource) -> Value {
    let mut key = Map::new();
    key.insert("Name".to_string(), Value::String(declared_name(resource)));
    if let Some(scope) = property(resource, WAFV2_SCOPE) {
        key.insert(WAFV2_SCOPE.to_string(), Value::String(scope));
    }
    key.insert(
        "Id".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    Value::Object(key)
}

/// The same document with every member the two sides spell DIFFERENTLY put in `spelling`'s
/// spelling, and everything else — the rules around a statement, the statements it is
/// nested in — carried through as it stands.
///
/// Two members are in that position, and both sit inside a rule's statement tree:
///
/// * a byte-match statement's search value, the one place a WAFv2 rule tree holds a BLOB
///   (`ByteMatchStatement$SearchString`, "a string value that you want WAF to search for"),
///   which travels base64-encoded on the awsJson wire while a template declares the bytes
///   themselves ([`wafv2_search_string_encoded`], [`wafv2_search_string_decoded`]);
/// * the ARN a reference statement names another entity by, which the API declares as `ARN`
///   and the resource types write as `Arn` ([`wafv2_reference_arn`]).
///
/// Both directions of each difference are this walk's business, so the request a declaration
/// becomes and the configuration the entity reads back as are spelled the way each side
/// spells them — a rule naming an IP set reaches that API under the member it requires
/// rather than being refused for a member the call did not carry.
///
/// `depth` is how many levels are already above this one, so the outermost call passes `0`;
/// a document deeper than [`MAX_PROPERTY_NESTING`] is refused rather than walked.
pub(super) fn wafv2_spelled(
    value: &Value,
    depth: usize,
    spelling: Wafv2Spelling,
) -> Result<Value, Refusal> {
    if depth >= MAX_PROPERTY_NESTING {
        return Err(too_deeply_nested());
    }
    match value {
        Value::Object(members) => {
            let mut rewritten = Map::new();
            for (key, member) in members {
                let member = match (key.as_str(), member) {
                    (WAFV2_BYTE_MATCH_STATEMENT, Value::Object(statement)) => {
                        // The statement's own members carry no further statement, so a
                        // rewritten one is not descended into.
                        Value::Object(match spelling {
                            Wafv2Spelling::Wire => wafv2_search_string_encoded(statement),
                            Wafv2Spelling::Template => wafv2_search_string_decoded(statement),
                        })
                    }
                    (name, Value::Object(statement))
                        if WAFV2_REFERENCE_STATEMENTS.contains(&name) =>
                    {
                        // The ARN member is renamed and the statement's REMAINING members
                        // (a forwarded-IP configuration, the overrides of a referenced
                        // group, a field to match) are carried through the same walk, none
                        // of them being a reference statement itself.
                        let renamed = Value::Object(wafv2_reference_arn(statement, spelling));
                        wafv2_spelled(&renamed, depth + 1, spelling)?
                    }
                    _ => wafv2_spelled(member, depth + 1, spelling)?,
                };
                rewritten.insert(key.clone(), member);
            }
            Ok(Value::Object(rewritten))
        }
        Value::Array(items) => {
            let mut rewritten = Vec::with_capacity(items.len());
            for item in items {
                rewritten.push(wafv2_spelled(item, depth + 1, spelling)?);
            }
            Ok(Value::Array(rewritten))
        }
        scalar => Ok(scalar.clone()),
    }
}

/// One reference statement with the ARN it names another entity by under the member
/// `spelling`'s side names it.
///
/// Towards the API the template's spelling is what is renamed, since the member the call
/// requires is [`WAFV2_REFERENCE_ARN`]; a document that already carries that member keeps
/// its value, so a declaration written in the API's own spelling is not rewritten by a
/// template-spelled one beside it.
///
/// Towards a TEMPLATE both spellings are reported, exactly as a byte-match statement's
/// search value is ([`wafv2_search_string_decoded`]): the drift comparison compares only a
/// member the declaration itself carries, so an entity nobody changed is not reported as
/// modified whichever of them its declaration used.
pub(super) fn wafv2_reference_arn(
    statement: &Map<String, Value>,
    spelling: Wafv2Spelling,
) -> Map<String, Value> {
    let mut rewritten = statement.clone();
    match spelling {
        Wafv2Spelling::Wire => {
            if let Some(declared) = rewritten.remove(WAFV2_TEMPLATE_REFERENCE_ARN) {
                if !rewritten.contains_key(WAFV2_REFERENCE_ARN) {
                    rewritten.insert(WAFV2_REFERENCE_ARN.to_string(), declared);
                }
            }
        }
        Wafv2Spelling::Template => {
            if let Some(reported) = rewritten.get(WAFV2_REFERENCE_ARN).cloned() {
                rewritten.insert(WAFV2_TEMPLATE_REFERENCE_ARN.to_string(), reported);
            }
        }
    }
    rewritten
}

/// One byte-match statement as the API takes it: its search value base64-encoded, which is
/// how the awsJson wire carries the blob `SearchString` is.
///
/// A template declares that value in one of two spellings — the bytes as text, or the
/// already-encoded form for bytes that are not text — and both name the same blob, so the
/// encoded spelling goes through as it is and the text one is encoded here. The codec is
/// `aws_smithy_types::base64`, the one the wire serializers of this workspace use.
///
/// A declaration whose encoded spelling is not a scalar is NOT one of the two, and is left
/// exactly as written: WAF's own validation of the member it then does not carry is what
/// judges it, rather than this walk dropping the value a template did declare.
pub(super) fn wafv2_search_string_encoded(statement: &Map<String, Value>) -> Map<String, Value> {
    let mut rewritten = statement.clone();
    let already_encoded = rewritten
        .get(WAFV2_SEARCH_STRING_BASE64)
        .and_then(scalar_text);
    if let Some(encoded) = already_encoded {
        rewritten.remove(WAFV2_SEARCH_STRING_BASE64);
        rewritten.insert(WAFV2_SEARCH_STRING.to_string(), Value::String(encoded));
        return rewritten;
    }
    if let Some(declared) = rewritten.get(WAFV2_SEARCH_STRING).and_then(scalar_text) {
        rewritten.insert(
            WAFV2_SEARCH_STRING.to_string(),
            Value::String(::aws_smithy_types::base64::encode(declared)),
        );
    }
    rewritten
}

/// One byte-match statement as a TEMPLATE spells it: the search value in BOTH spellings a
/// template can declare it in, so the drift comparison — which compares only a member the
/// declaration itself carries — finds it in the spelling that declaration wrote, and an
/// entity nobody changed is not reported as modified.
///
/// The encoded spelling is always reported, being the one the wire carried. The text
/// spelling is reported beside it where the bytes ARE text; bytes that are not text have no
/// text spelling, which is why a template declaring them has to use the encoded one.
pub(super) fn wafv2_search_string_decoded(statement: &Map<String, Value>) -> Map<String, Value> {
    let mut rewritten = statement.clone();
    let Some(encoded) = rewritten.get(WAFV2_SEARCH_STRING).and_then(scalar_text) else {
        return rewritten;
    };
    let decoded = ::aws_smithy_types::base64::decode(&encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    rewritten.insert(
        WAFV2_SEARCH_STRING_BASE64.to_string(),
        Value::String(encoded),
    );
    match decoded {
        Some(text) => {
            rewritten.insert(WAFV2_SEARCH_STRING.to_string(), Value::String(text));
        }
        None => {
            rewritten.remove(WAFV2_SEARCH_STRING);
        }
    }
    rewritten
}

/// Call one WAFv2 operation on the awsJson1_1 wire its model declares.
pub(super) async fn call_wafv2(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        WAFV2_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{WAFV2_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS WAF (WAFv2): AWS::WAFv2::WebACLAssociation
// ---------------------------------------------------------------------------

/// The two properties an `AWS::WAFv2::WebACLAssociation` binds, which are also the two
/// members `AssociateWebACL` takes under exactly these names: the protected resource and
/// the web ACL to put in front of it.
pub(super) const WAFV2_ASSOCIATION_RESOURCE_ARN: &str = "ResourceArn";
pub(super) const WAFV2_ASSOCIATION_WEB_ACL_ARN: &str = "WebACLArn";

/// The calls that make, remove and read back a binding between a protected resource and a
/// web ACL.
pub(super) const WAFV2_ASSOCIATE: &str = "AssociateWebACL";
pub(super) const WAFV2_DISASSOCIATE: &str = "DisassociateWebACL";
pub(super) const WAFV2_GET_FOR_RESOURCE: &str = "GetWebACLForResource";

/// The member `GetWebACLForResource` answers the web ACL in front of a resource under, and
/// the member of that web ACL carrying the ARN a declaration names it by.
pub(super) const WAFV2_WEB_ACL_MEMBER: &str = "WebACL";
pub(super) const WAFV2_ARN: &str = "ARN";

/// Put the web ACL an `AWS::WAFv2::WebACLAssociation` declares in front of the resource it
/// names.
///
/// This type is no entity of AWS WAF's own, so there is nothing to create: its two
/// properties are `AssociateWebACL`'s own two members, and that call is the whole
/// deployment of one. WAF is also what JUDGES the pair — "your request references an ARN
/// that is malformed, or corresponds to a resource with which a web ACL can't be
/// associated" is its `WAFInvalidParameterException`, and a `WebACLArn` naming no web ACL
/// its `WAFNonexistentItemException` — so a declaration naming a resource kind a web ACL
/// can never front is refused by that service and the resource fails, rather than being
/// recorded as a protection nothing could apply.
///
/// What the resource IS, is the pair it bound, so the physical id answered here is that
/// pair ([`super::template::composite_identity`]) read off the document the call was made
/// with: a reference to a web ACL of the same stack carries the ARN AWS WAF minted for it,
/// which no naming rule could have computed.
pub(super) async fn associate_web_acl(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert(
        WAFV2_ASSOCIATION_RESOURCE_ARN.to_string(),
        Value::String(required_property(
            resource,
            WAFV2_ASSOCIATION_RESOURCE_ARN,
        )?),
    );
    request.insert(
        WAFV2_ASSOCIATION_WEB_ACL_ARN.to_string(),
        Value::String(required_property(resource, WAFV2_ASSOCIATION_WEB_ACL_ARN)?),
    );
    call_wafv2(ctx, region, WAFV2_ASSOCIATE, &Value::Object(request)).await?;
    // The pair is computed again HERE, from the document the call was made with, because
    // this pass is the one that owns the id: the declaration was resolved before AWS WAF
    // had minted the web ACL's ARN, so the pair
    // [`super::template::resolve_properties`] stamped on the record carries the stand-in
    // the naming rules computed, and the pair this resource really bound is the one the
    // re-pointed document names.
    Ok(Created {
        physical_resource_id: super::template::composite_identity(
            &resource.resource_type,
            &resource.properties,
        ),
        attributes: BTreeMap::new(),
    })
}

/// Take the binding a stack made away again.
///
/// `DisassociateWebACL` addresses a binding by the PROTECTED resource alone — a resource
/// carries at most one web ACL in front of it — and what this resource IS, is the PAIR it
/// bound: so the binding is read back first and taken away only while the web ACL in front
/// of the resource is still the one this record bound. A resource carrying another web ACL,
/// or none at all, no longer carries the pair this record names — it is already in the state
/// the delete asks for, and disassociating it would take away a binding this record never
/// made, which is exactly what a replacement's cleanup must not do.
pub(super) async fn disassociate_web_acl(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let resource_arn = required_property(resource, WAFV2_ASSOCIATION_RESOURCE_ARN)?;
    let web_acl_arn = required_property(resource, WAFV2_ASSOCIATION_WEB_ACL_ARN)?;
    if web_acl_in_front_of(ctx, region, &resource_arn).await? != Some(web_acl_arn) {
        return Ok(());
    }
    let mut request = Map::new();
    request.insert(
        WAFV2_ASSOCIATION_RESOURCE_ARN.to_string(),
        Value::String(resource_arn),
    );
    match call_wafv2(ctx, region, WAFV2_DISASSOCIATE, &Value::Object(request)).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == WAFV2_NONEXISTENT_ITEM => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The binding as AWS WAF reports it, for drift detection: the protected resource the
/// declaration names, and the ARN of the web ACL that service has in front of it NOW — so a
/// binding re-pointed outside the stack is reported as modified and one taken away as
/// deleted.
pub(super) async fn describe_web_acl_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let resource_arn = required_property(resource, WAFV2_ASSOCIATION_RESOURCE_ARN)?;
    let Some(web_acl_arn) = web_acl_in_front_of(ctx, region, &resource_arn).await? else {
        return Ok(None);
    };
    let mut reported = Map::new();
    reported.insert(
        WAFV2_ASSOCIATION_RESOURCE_ARN.to_string(),
        Value::String(resource_arn),
    );
    reported.insert(
        WAFV2_ASSOCIATION_WEB_ACL_ARN.to_string(),
        Value::String(web_acl_arn),
    );
    Ok(Some(Value::Object(reported)))
}

/// The ARN of the web ACL AWS WAF has in front of `resource_arn` now, or `None` for a
/// resource nothing is in front of.
///
/// `GetWebACLForResource` is the one read of a binding, and a resource with none is not an
/// error: "the web ACL that is associated with the resource. If there is no associated
/// resource, WAF returns a null web ACL" (`GetWebACLForResourceResponse$WebACL`). A resource
/// that service no longer holds at all answers its own not-found code, which is the same
/// absence.
pub(super) async fn web_acl_in_front_of(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_arn: &str,
) -> Result<Option<String>, Refusal> {
    let mut request = Map::new();
    request.insert(
        WAFV2_ASSOCIATION_RESOURCE_ARN.to_string(),
        Value::String(resource_arn.to_string()),
    );
    let answered =
        match call_wafv2(ctx, region, WAFV2_GET_FOR_RESOURCE, &Value::Object(request)).await {
            Ok(answered) => answered,
            Err(refusal) if refusal.code == WAFV2_NONEXISTENT_ITEM => return Ok(None),
            Err(refusal) => return Err(refusal),
        };
    Ok(answered
        .get(WAFV2_WEB_ACL_MEMBER)
        .and_then(|web_acl| web_acl.get(WAFV2_ARN))
        .and_then(scalar_text))
}

// ---------------------------------------------------------------------------
// Amazon GuardDuty: AWS::GuardDuty::Detector
// ---------------------------------------------------------------------------

/// Amazon GuardDuty's signing name, from its model (`sigv4Name: guardduty`,
/// `endpointPrefix: guardduty`). GuardDuty speaks `restJson1`, so its operations are
/// addressed by their `@http` METHOD and URI rather than by an `X-Amz-Target`.
pub(super) const GUARDDUTY_SIGNING_NAME: &str = "guardduty";

/// The URI `CreateDetector` is bound to, and the collection a detector's own URI hangs
/// off: `POST /detector` creates one, `GET /detector/{DetectorId}` reads it and
/// `DELETE /detector/{DetectorId}` deletes it
/// (`smithy_query(operation="CreateDetector,GetDetector,DeleteDetector")`).
pub(super) const GUARDDUTY_DETECTOR_URI: &str = "/detector";

/// The error classes GuardDuty answers a client rejection under.
///
/// GuardDuty models ONE client error (`BadRequestException`, httpError 400) and answers
/// with the specific class in the body's `__type`
/// (`services/service-com-amazonaws-guardduty-guarddutyapiservice/src/scaffolded.rs`), so
/// the code alone does not distinguish a detector id that names no detector of this
/// account from any other rejection — which is why [`delete_detector`] confirms one by
/// reading the detector back rather than reading "already deleted" off the code.
pub(super) const GUARDDUTY_INVALID_INPUT: &str = "InvalidInputException";
pub(super) const GUARDDUTY_BAD_REQUEST: &str = "BadRequestException";
