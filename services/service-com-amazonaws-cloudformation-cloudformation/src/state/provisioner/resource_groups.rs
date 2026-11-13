// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Resource Groups: the resource group an
//! `AWS::ResourceGroups::Group` declares. One file of [`super`]'s per-type arms, dispatched
//! to by its `Provisioner`.
//!
//! A resource group is a NAMED resource of that service — "Ref returns the group name", and
//! every Resource Groups operation on one takes that name or the ARN it is minted under — so
//! the name CloudFormation decided is what `CreateGroup` creates the group as and what
//! `DeleteGroup` removes. The ARN the stack publishes is the one Resource Groups minted for
//! it, read off that create's own answer, which is also what keeps the `Arn` a stack's
//! outputs carry inside the group-ARN form that service's own `GroupArnV2` fixes.
//!
//! The type's properties are `CreateGroup`'s own members, named identically on both sides,
//! with two differences and no others:
//!
//! * `ResourceQuery$Query` is a template DOCUMENT where the request takes the JSON TEXT of
//!   one — that member is a string on the wire ("a `ResourceQuery` specifies both a query
//!   `Type` and a `Query` string as JSON string objects") — so the declared document is sent
//!   as its text ([`resource_query_member`]), and a declaration that already states the text
//!   is passed through as written;
//! * `Tags` is the `[{Key, Value}]` list a template writes where that create takes a MAP, so
//!   the declared tags travel beside the ones CloudFormation gives the resource
//!   ([`tags_of`]) as the map its own `Tags` member takes;
//! * `Resources` is no member of the create at all: the static members of a group are added
//!   by the `GroupResources` a client makes next, so the arm makes that second call
//!   ([`group_declared_resources`]) and fails the resource on an entry that service reports
//!   as failed.

use super::*;

/// Resource Groups' signing name, from its model (`sigv4Name: resource-groups`, the same
/// string as its `endpointPrefix` and its `arnNamespace`). It speaks `restJson1`, so its
/// calls ride on [`call_rest_json_named`], each on the URI its own operation's `@http`
/// binding declares.
pub(super) const RESOURCE_GROUPS_SIGNING_NAME: &str = "resource-groups";

/// The URIs the Resource Groups operations a stack makes are bound to, as that model's
/// `@http` traits declare them: `POST /groups` creates a group, `POST /get-group` and
/// `POST /delete-group` read and remove one, `POST /update-group` and
/// `POST /update-group-query` change one, and `POST /group-resources` adds static members.
const CREATE_GROUP_URI: &str = "/groups";
const GET_GROUP_URI: &str = "/get-group";
const DELETE_GROUP_URI: &str = "/delete-group";
const UPDATE_GROUP_URI: &str = "/update-group";
const UPDATE_GROUP_QUERY_URI: &str = "/update-group-query";
const GROUP_RESOURCES_URI: &str = "/group-resources";

/// The URI that model binds its three tagging operations to
/// (`GET|PUT|PATCH /resources/{Arn}/tags`), each addressing the group by the ARN Resource
/// Groups minted for it.
const GROUP_TAGS_PREFIX: &str = "/resources";
const GROUP_TAGS_SUFFIX: &str = "tags";

/// The operations this module calls, as Resource Groups' own model names them.
const CREATE_GROUP: &str = "CreateGroup";
const GET_GROUP: &str = "GetGroup";
const DELETE_GROUP: &str = "DeleteGroup";
const UPDATE_GROUP: &str = "UpdateGroup";
const UPDATE_GROUP_QUERY: &str = "UpdateGroupQuery";
const GROUP_RESOURCES: &str = "GroupResources";
const GET_TAGS: &str = "GetTags";
const TAG: &str = "Tag";
const UNTAG: &str = "Untag";

/// The error code Resource Groups answers for a group it does not hold
/// (`NotFoundException`, the modelled 404 of its reads, its deletes and its updates).
const RESOURCE_GROUPS_NOT_FOUND: &str = "NotFoundException";

/// The properties `AWS::ResourceGroups::Group` publishes, which is what this arm carries to
/// Resource Groups: a property outside the list is REFUSED rather than dropped, so a group
/// is never reported created with less than its declaration asked for.
const RESOURCE_GROUP_PROPERTIES: &[&str] = &[
    "Name",
    "Description",
    "ResourceQuery",
    "Configuration",
    "Resources",
    "Tags",
];

/// The properties of a declaration that reach the group through a call of their own — the
/// name it is created under, its tags, its description and its resource query — so the rest
/// are the ones no call can change on a group that already exists
/// ([`group_unchangeable_properties`]).
const RESOURCE_GROUP_CHANGEABLE_PROPERTIES: &[&str] =
    &["Name", "Tags", "Description", "ResourceQuery"];

/// The member each call addresses one group by: `Group`, which "can be the group's name or
/// its ARN" (`GroupStringV2`).
const GROUP_MEMBER: &str = "Group";

/// The members of the `Group` structure every read of one reports, and the structure itself.
const GROUP_STRUCTURE: &str = "Group";
const GROUP_ARN_MEMBER: &str = "GroupArn";
const GROUP_NAME_MEMBER: &str = "Name";
const GROUP_DESCRIPTION_MEMBER: &str = "Description";

/// The members of the `ResourceQuery` structure both sides spell: its `Type` and the `Query`
/// the wire carries as TEXT.
const QUERY_TYPE_MEMBER: &str = "Type";
const QUERY_MEMBER: &str = "Query";

/// The members a `GroupResources` call takes and the ones its answer reports a failed entry
/// under.
const RESOURCE_ARNS_MEMBER: &str = "ResourceArns";
const FAILED_MEMBER: &str = "Failed";
const ERROR_CODE_MEMBER: &str = "ErrorCode";
const ERROR_MESSAGE_MEMBER: &str = "ErrorMessage";

/// The members the tagging calls take: `Tags` is the MAP that service holds a group's tags
/// as, and `Keys` the list `Untag` drops.
const TAGS_MEMBER: &str = "Tags";
const TAG_KEYS_MEMBER: &str = "Keys";

/// Call one Resource Groups operation on the `restJson1` wire its model declares, naming the
/// operation so a refusal is reported the way that service's own SDK raises it.
async fn call_resource_groups(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    method: &'static str,
    uri: &str,
    body: Option<&Value>,
) -> Result<Value, Refusal> {
    call_rest_json_named(
        ctx,
        RESOURCE_GROUPS_SIGNING_NAME,
        region,
        operation,
        method,
        uri,
        body,
    )
    .await
}

/// Create the resource group an `AWS::ResourceGroups::Group` declares, so a client that
/// deployed the stack finds the group in Resource Groups itself — `ListGroups` reports it and
/// `GetGroup` answers with it — rather than that service's `NotFoundException` for a group
/// nothing created.
///
/// The group is created under the name CloudFormation decided, which is this resource's
/// physical id and the identifier the type's `Ref` returns, and the ARN Resource Groups
/// minted for it is reported back as the `Arn` attribute, so the handle the stack publishes
/// is the one that service really holds the group under.
pub(super) async fn create_resource_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, RESOURCE_GROUP_PROPERTIES)?;
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        GROUP_NAME_MEMBER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    if let Some(description) = properties.get(GROUP_DESCRIPTION_MEMBER) {
        request.insert(
            GROUP_DESCRIPTION_MEMBER.to_string(),
            Value::String(attribute_text(description)),
        );
    }
    if let Some(query) = properties.get("ResourceQuery") {
        request.insert("ResourceQuery".to_string(), resource_query_member(query)?);
    }
    // A group's service configuration is declared as the `[{Type, Parameters}]` list the
    // create's own `Configuration` member takes, member for member, so it travels as written.
    if let Some(configuration) = properties.get("Configuration") {
        request.insert("Configuration".to_string(), configuration.clone());
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert(TAGS_MEMBER.to_string(), Value::Object(tags));
    }
    let answered = call_resource_groups(
        ctx,
        region,
        CREATE_GROUP,
        "POST",
        CREATE_GROUP_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = reported_group_arn(&answered).ok_or_else(|| Refusal {
        code: "InternalFailure".to_string(),
        message: "Resource Groups answered CreateGroup without a GroupArn".to_string(),
    })?;
    group_declared_resources(ctx, region, &arn, properties.get("Resources")).await?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn);
    Ok(Created::attributes(attributes))
}

/// The ARN Resource Groups reports for the group its answer carries, or `None` for an answer
/// that identifies no group at all.
fn reported_group_arn(answered: &Value) -> Option<String> {
    answered
        .get(GROUP_STRUCTURE)?
        .get(GROUP_ARN_MEMBER)
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .map(ToString::to_string)
}

/// The `ResourceQuery` a declaration states, as the structure Resource Groups takes.
///
/// `Type` is the same enum value on both sides. `Query` is a STRING on the wire while a
/// template declares the document itself (`{"ResourceTypeFilters": …, "TagFilters": …}`), so
/// a declared document is sent as its JSON text and a declaration that already states the
/// text is passed through as written — the same correspondence an `AWS::Events::Rule`'s
/// `EventPattern` has with the JSON text `PutRule` takes.
fn resource_query_member(declared: &Value) -> Result<Value, Refusal> {
    let Some(members) = declared.as_object() else {
        return Err(Refusal {
            code: "ValidationException".to_string(),
            message: "the ResourceQuery property of an AWS::ResourceGroups::Group states a \
                      Type and a Query"
                .to_string(),
        });
    };
    let mut query = Map::new();
    if let Some(query_type) = members.get(QUERY_TYPE_MEMBER) {
        query.insert(
            QUERY_TYPE_MEMBER.to_string(),
            Value::String(attribute_text(query_type)),
        );
    }
    if let Some(declared_query) = members.get(QUERY_MEMBER) {
        let text = match declared_query {
            Value::String(text) => text.clone(),
            document => serde_json::to_string(document).map_err(|error| Refusal {
                code: "SerializationException".to_string(),
                message: error.to_string(),
            })?,
        };
        query.insert(QUERY_MEMBER.to_string(), Value::String(text));
    }
    Ok(Value::Object(query))
}

/// Add the static members a declaration's `Resources` list asks for to the group just
/// created, through the `GroupResources` a client makes next: that list is no member of the
/// create, so this is the one call that records a group's static membership.
///
/// An entry Resource Groups reports under `Failed` is a member it did NOT add, so the
/// resource fails with that service's own code and message rather than being reported created
/// with a membership it does not have. A declaration that states no resources asks nothing.
async fn group_declared_resources(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group: &str,
    declared: Option<&Value>,
) -> Result<(), Refusal> {
    let arns: Vec<Value> = declared
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|arn| Value::String(attribute_text(arn)))
                .collect()
        })
        .unwrap_or_default();
    if arns.is_empty() {
        return Ok(());
    }
    let answered = call_resource_groups(
        ctx,
        region,
        GROUP_RESOURCES,
        "POST",
        GROUP_RESOURCES_URI,
        Some(&serde_json::json!({
            GROUP_MEMBER: group,
            RESOURCE_ARNS_MEMBER: Value::Array(arns),
        })),
    )
    .await?;
    let failed = answered.get(FAILED_MEMBER).and_then(Value::as_array);
    let Some(first) = failed.and_then(|entries| entries.first()) else {
        return Ok(());
    };
    Err(Refusal {
        code: first
            .get(ERROR_CODE_MEMBER)
            .and_then(Value::as_str)
            .unwrap_or("InternalFailure")
            .to_string(),
        message: first
            .get(ERROR_MESSAGE_MEMBER)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Delete the resource group a stack created, addressed by its name — this resource's
/// physical id. A group Resource Groups no longer holds (`NotFoundException`, judged on the
/// CODE and never on a message that carries the request's own name) is already in the state
/// the delete asks for.
pub(super) async fn delete_resource_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_resource_groups(
        ctx,
        region,
        DELETE_GROUP,
        "POST",
        DELETE_GROUP_URI,
        Some(&serde_json::json!({ GROUP_MEMBER: physical_resource_id })),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_GROUPS_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// `GetGroup`'s own record of the group of `name`, or `None` for one Resource Groups no longer
/// holds (`NotFoundException`, judged on the CODE).
///
/// The one read of a group in this module, so the drift comparison below and the ARN the
/// tagging calls address it by ([`retag_resource_group`]) come from the same answer rather
/// than from two spellings of the same request.
async fn read_resource_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    match call_resource_groups(
        ctx,
        region,
        GET_GROUP,
        "POST",
        GET_GROUP_URI,
        Some(&serde_json::json!({ GROUP_MEMBER: name })),
    )
    .await
    {
        Ok(answer) => Ok(answer.get(GROUP_STRUCTURE).cloned()),
        Err(refusal) if refusal.code == RESOURCE_GROUPS_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The group's configuration as Resource Groups reports it, for drift detection: `GetGroup`'s
/// own record. One it no longer holds (`NotFoundException`) was deleted.
///
/// The properties that read reports under the names this type declares are the group's `Name`
/// and its `Description`; its query, its configuration and its tags are each answered by a
/// call of their own, and a property the read does not report is not evidence of anything
/// ([`super::drift`]).
pub(super) async fn describe_resource_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_resource_group(ctx, region, physical_resource_id)
        .await?
        .map(|group| {
            reported_as_properties(
                &group,
                &[
                    (GROUP_NAME_MEMBER, GROUP_NAME_MEMBER),
                    (GROUP_DESCRIPTION_MEMBER, GROUP_DESCRIPTION_MEMBER),
                ],
            )
        }))
}

/// One group declaration less the properties a call of their own carries: the half of it no
/// Resource Groups call can change on a group that already exists, so a declaration that
/// changes one of these is reported as NOT applied rather than as a success that dropped it.
fn group_unchangeable_properties(properties: &Value) -> Map<String, Value> {
    match properties {
        Value::Object(object) => object
            .iter()
            .filter(|(member, _)| {
                !RESOURCE_GROUP_CHANGEABLE_PROPERTIES.contains(&member.as_str())
            })
            .map(|(member, value)| (member.clone(), value.clone()))
            .collect(),
        _ => Map::new(),
    }
}

/// Apply a CHANGED `AWS::ResourceGroups::Group` declaration to the group the stack already
/// has, through the two calls Resource Groups publishes for a group that exists.
///
/// `UpdateGroup` "updates the description for an existing group" and `UpdateGroupQuery`
/// "updates the resource query of a group", so a changed description or a changed query
/// reaches the group the stack created rather than replacing it. The group's `Name` is its
/// identity — "you cannot update the name of a resource group" — and a change to it replaces
/// the resource instead of reaching here; its `Tags` are no member of either call and travel
/// through that service's own tagging calls ([`retag_resource_group`]). A declaration that
/// changes any other property, or that drops a query no call can remove (`UpdateGroupQuery`'s
/// `ResourceQuery` is `@required`), is reported as not applied.
pub(super) async fn update_resource_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if group_unchangeable_properties(&resource.properties) != group_unchangeable_properties(declared)
    {
        return Ok(None);
    }
    let group = resource.physical_resource_id.as_str();
    let held_description = resource.properties.get(GROUP_DESCRIPTION_MEMBER);
    let declared_description = declared.get(GROUP_DESCRIPTION_MEMBER);
    if held_description != declared_description {
        // A declaration that drops the description asks for the empty one, which is the
        // value a group created without one carries (`Description` has no minimum length).
        call_resource_groups(
            ctx,
            region,
            UPDATE_GROUP,
            "POST",
            UPDATE_GROUP_URI,
            Some(&serde_json::json!({
                GROUP_MEMBER: group,
                GROUP_DESCRIPTION_MEMBER: declared_description.map(attribute_text).unwrap_or_default(),
            })),
        )
        .await?;
    }
    let held_query = resource.properties.get("ResourceQuery");
    let declared_query = declared.get("ResourceQuery");
    if held_query != declared_query {
        let Some(query) = declared_query else {
            return Ok(None);
        };
        call_resource_groups(
            ctx,
            region,
            UPDATE_GROUP_QUERY,
            "POST",
            UPDATE_GROUP_QUERY_URI,
            Some(&serde_json::json!({
                GROUP_MEMBER: group,
                "ResourceQuery": resource_query_member(query)?,
            })),
        )
        .await?;
    }
    Ok(Some(Created::default()))
}

/// Bring the tags of the group a resource record names to the ones this deployment gives it,
/// through Resource Groups' own `Tag` and `Untag`.
///
/// The set is the declaration's own `Tags` beside the ones CloudFormation propagates
/// ([`tags_of`]) — the same set the CREATE applied — so a key outside it is one this
/// deployment no longer gives the resource and is removed, and one in it with another value is
/// re-valued. Where the two already agree nothing is called at all, which is what makes this
/// safe to ask on every deployment.
///
/// Those calls address a group by its ARN, which is the handle that service minted rather than
/// one rebuilt here, so it is read back from the group itself ([`read_resource_group`]). A
/// group Resource Groups no longer holds has no tags to reconcile.
pub(super) async fn retag_resource_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let Some(arn) = read_resource_group(ctx, region, &resource.physical_resource_id)
        .await?
        .as_ref()
        .and_then(|group| group.get(GROUP_ARN_MEMBER))
        .and_then(Value::as_str)
        .map(ToString::to_string)
    else {
        return Ok(());
    };
    let uri = format!(
        "{GROUP_TAGS_PREFIX}/{}/{GROUP_TAGS_SUFFIX}",
        percent_encoded(&arn)
    );
    // That service answers its tag read as the MAP of key to value a declaration's `Tags`
    // property may also be written as, so one reader serves both sides.
    let held = declared_tags(&call_resource_groups(ctx, region, GET_TAGS, "GET", &uri, None).await?);
    let desired = tags_of(declared, propagated);
    let (added, removed) = tag_delta(&held, &desired);
    if !added.is_empty() {
        call_resource_groups(
            ctx,
            region,
            TAG,
            "PUT",
            &uri,
            Some(&serde_json::json!({ TAGS_MEMBER: Value::Object(tags_as_map(&added)) })),
        )
        .await?;
    }
    if !removed.is_empty() {
        call_resource_groups(
            ctx,
            region,
            UNTAG,
            "PATCH",
            &uri,
            Some(&serde_json::json!({ TAG_KEYS_MEMBER: removed })),
        )
        .await?;
    }
    Ok(())
}
