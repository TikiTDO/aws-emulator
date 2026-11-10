// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amazon EventBridge Pipes: the PIPE a stack declares,
//! which connects an event source to a target. One file of [`super`]'s per-type arms,
//! dispatched to by its `Provisioner`.
//!
//! `AWS::Pipes::Pipe`'s properties ARE `CreatePipe`'s own members, spelled identically on
//! both sides (`Source`, `SourceParameters`, `Enrichment`, `EnrichmentParameters`, `Target`,
//! `TargetParameters`, `RoleArn`, `Description`, `DesiredState`, `KmsKeyIdentifier`,
//! `LogConfiguration`, `Tags`), so the evaluated property document IS the request — with the
//! two members this emulator decides set on the way out: the pipe's NAME, which that call
//! takes in its URI (`POST /v1/pipes/{Name}`) and which is this resource's physical id, and
//! the tag map, which is the declaration's own tags beside the ones CloudFormation
//! propagates ([`tags_of`]).
//!
//! The identity is CloudFormation's: "Ref returns the pipe name", which is the physical id
//! the naming rules compute from the declaration's `Name` property, so the type is not in
//! [`super::template::identity_is_service_assigned`]. What the type publishes BEYOND that
//! name is EventBridge Pipes' own to report — the pipe's ARN, the state it is in, why it is
//! in that state, and the two instants it records — so those are read off the answers that
//! service gives ([`pipe_attributes`]) rather than computed here.
//!
//! `CreatePipe` is ASYNCHRONOUS: it answers the pipe `CREATING`, in which it moves no record
//! yet, and the pipe reaches the state its `DesiredState` asked for a moment later. So the
//! resource's creation is held open until that service reports the pipe resting
//! ([`readiness`]), and the attributes the stack publishes for it are re-read then
//! ([`ready_attributes`]) — which is what makes the `CurrentState` a stack's output carries
//! the one `pipes DescribePipe` answers with for the same pipe.

use super::*;

/// EventBridge Pipes' signing name, from its model (`sigv4Name: pipes`, service shape
/// `Pipes`), which speaks `restJson1` — so its calls ride on [`call_rest_json`], each on the
/// URI its own operation's `@http` binding declares.
pub(super) const PIPES_SIGNING_NAME: &str = "pipes";

/// The URI the pipe operations a stack makes are bound to, as that model's `@http` traits
/// declare them: `POST|GET|PUT|DELETE /v1/pipes/{Name}`.
pub(super) const PIPES_URI: &str = "/v1/pipes";

/// The error code EventBridge Pipes answers for a pipe it does not hold
/// (`NotFoundException`, the modelled 404 of its reads, its updates and its deletes).
pub(super) const PIPES_NOT_FOUND: &str = "NotFoundException";

/// The `PipeState` values a pipe RESTS in — the two a pipe reaches when a creation or a
/// change has been carried out, out of that model's `PipeState` enum. Every other
/// non-failure value of it is a state a move already in flight has not landed from.
const PIPE_RESTING_STATES: &[&str] = &["RUNNING", "STOPPED"];

/// The suffix the `PipeState` values that report a move that CANNOT be completed carry
/// (`CREATE_FAILED`, `UPDATE_FAILED`, `START_FAILED`, …): a pipe in one of them is not
/// coming up, so waiting it out would report a resource as created that is not.
const PIPE_FAILED_STATE_SUFFIX: &str = "_FAILED";

/// The members of a pipe's own `Properties` that `CreatePipe` does NOT take as members of
/// its request document: the `Name` it binds to its URI and the `Tags` this arm rebuilds
/// from the declaration's own tags and the ones CloudFormation propagates.
const PIPE_MEMBERS_SET_HERE: &[&str] = &["Name", "Tags"];

/// The members `UpdatePipe` does not take beyond those: a pipe's `Source` is create-only on
/// that API (the call takes no `Source` member at all) and no tag reaches it either.
const PIPE_MEMBERS_NOT_UPDATED: &[&str] = &["Name", "Tags", "Source"];

/// Create the pipe an `AWS::Pipes::Pipe` declares, so a client that deployed the stack and
/// then calls `pipes DescribePipe` with the name the stack publishes reads the pipe back
/// rather than a `NotFoundException`.
///
/// The request is the evaluated property document itself ([`pipe_request`]) — those
/// properties being `CreatePipe`'s own members — under the name CloudFormation decided, with
/// the tags the declaration asks for plus the ones CloudFormation gives the resource as the
/// tag MAP that call takes.
///
/// What comes back is the pipe as that service reports it right now, which is the resource's
/// attributes ([`pipe_attributes`]): the ARN it minted and the state the pipe is in. That
/// state is `CREATING`, so the resource's creation is not finished here — [`readiness`] is
/// what ends it.
pub(super) async fn create_pipe(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = pipe_request(&resource.properties, PIPE_MEMBERS_SET_HERE);
    let tags = tags_of(&resource.properties, propagated);
    // `CreatePipe$Tags` is a map with a `@length(min: 1)`: a pipe with no tag at all carries
    // the member absent rather than an empty map that call would refuse.
    if !tags.is_empty() {
        request.insert("Tags".to_string(), Value::Object(tags_as_map(&tags)));
    }
    let answered = call_rest_json(
        ctx,
        PIPES_SIGNING_NAME,
        region,
        "POST",
        &pipe_uri(&resource.physical_resource_id),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Created::attributes(pipe_attributes(&answered)))
}

/// Apply a CHANGED `AWS::Pipes::Pipe` declaration to the pipe the stack already has, through
/// that service's own `UpdatePipe` (`PUT /v1/pipes/{Name}`).
///
/// The request is the same document the create builds from a declaration, less the members
/// that call does not take ([`PIPE_MEMBERS_NOT_UPDATED`]): a pipe's `Source` is not among
/// `UpdatePipe`'s members — the resource that feeds a pipe is fixed when the pipe is created
/// — and neither are its tags, which that API changes through `TagResource`/`UntagResource`.
///
/// The pipe reports `UPDATING` while that service applies the change, so the resource is
/// held open on the same [`readiness`] a create waits on, and what the stack publishes for
/// it is what the update answered.
pub(super) async fn update_pipe(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let request = pipe_request(declared, PIPE_MEMBERS_NOT_UPDATED);
    let answered = call_rest_json(
        ctx,
        PIPES_SIGNING_NAME,
        region,
        "PUT",
        &pipe_uri(&resource.physical_resource_id),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Some(Created::attributes(pipe_attributes(&answered))))
}

/// Delete the pipe a stack created, addressed by its name — this resource's physical id. A
/// pipe EventBridge Pipes no longer holds (`NotFoundException`, judged on the CODE and never
/// on a message that carries the request's own name) is already in the state the delete asks
/// for.
pub(super) async fn delete_pipe(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = pipe_uri(physical_resource_id);
    match call_rest_json(ctx, PIPES_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == PIPES_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The pipe's configuration as EventBridge Pipes reports it, for drift detection:
/// `DescribePipe`'s own record. One it no longer holds (`NotFoundException`) was deleted.
///
/// Every member reported here is spelled exactly as this type's property is, so the
/// comparison is the declaration against what the service really holds. The pipe's
/// `LogConfiguration` is deliberately left out: that read answers the `PipeLogConfiguration`
/// shape rather than the `PipeLogConfigurationParameters` the type declares, so it is not
/// the same shape — and a property the read does not report is not evidence of anything
/// ([`super::drift`]).
pub(super) async fn describe_pipe(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_pipe(ctx, region, physical_resource_id)
        .await?
        .map(|pipe| {
            reported_as_properties(
                &pipe,
                &[
                    ("Description", "Description"),
                    ("DesiredState", "DesiredState"),
                    ("Enrichment", "Enrichment"),
                    ("EnrichmentParameters", "EnrichmentParameters"),
                    ("KmsKeyIdentifier", "KmsKeyIdentifier"),
                    ("RoleArn", "RoleArn"),
                    ("Source", "Source"),
                    ("SourceParameters", "SourceParameters"),
                    ("Target", "Target"),
                    ("TargetParameters", "TargetParameters"),
                ],
            )
        }))
}

/// Whether EventBridge Pipes reports the pipe this stack created or changed as settled.
///
/// "The state the pipe is in" (`Pipe$CurrentState`) is `CREATING` while that service brings
/// a pipe up and `UPDATING` while it applies a change, and in neither does the pipe carry
/// the configuration the declaration asked for yet — so the resource's creation is finished
/// once that service reports the pipe RESTING in one of the two states it rests in
/// ([`PIPE_RESTING_STATES`]), which is the state its `DesiredState` asked for.
///
/// A pipe that service reports in one of the FAILED states of that enum is not coming up at
/// all, so the resource fails with the `StateReason` the pipe carries — why it is in that
/// state — rather than waiting the window out.
pub(super) async fn readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let pipe = read_pipe(ctx, region, physical_resource_id)
        .await?
        .ok_or_else(|| Refusal {
            code: PIPES_NOT_FOUND.to_string(),
            message: format!(
                "EventBridge Pipes no longer holds the pipe {physical_resource_id} this stack \
                 created"
            ),
        })?;
    let state = pipe
        .get("CurrentState")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if PIPE_RESTING_STATES.contains(&state.as_str()) {
        return Ok(Readiness::Ready);
    }
    if state.ends_with(PIPE_FAILED_STATE_SUFFIX) {
        let reason = pipe
            .get("StateReason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
            .map_or_else(
                || format!("EventBridge Pipes reports the pipe {state}"),
                ToString::to_string,
            );
        return Ok(Readiness::Failed(reason));
    }
    Ok(Readiness::Pending(state))
}

/// The attributes EventBridge Pipes publishes for the pipe now that it reports it settled:
/// the state it came to rest in, why, and the instant it was last modified — none of which
/// the create's own answer could carry, because the pipe was still `CREATING` then.
pub(super) async fn ready_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    Ok(read_pipe(ctx, region, physical_resource_id)
        .await?
        .map(|pipe| pipe_attributes(&pipe))
        .unwrap_or_default())
}

/// `DescribePipe` for one pipe, or `None` for a pipe that service does not hold
/// (`NotFoundException`, judged on the CODE). The one read behind the drift comparison, the
/// readiness wait and the attributes read off a settled pipe, so none of the three can ask
/// for a pipe differently.
async fn read_pipe(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = pipe_uri(physical_resource_id);
    match call_rest_json(ctx, PIPES_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(pipe) => Ok(Some(pipe)),
        Err(refusal) if refusal.code == PIPES_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI that addresses ONE pipe: its name in the path, which is the single member every
/// pipe operation binds to its URI (`/v1/pipes/{Name}`).
fn pipe_uri(name: &str) -> String {
    format!("{PIPES_URI}/{}", percent_encoded(name))
}

/// The request body a pipe call takes for one declaration: the evaluated property document
/// itself, less the members the call does not take as part of its document (`without`).
///
/// The one place a declaration becomes a request, so the create and the in-place update of
/// the same declaration cannot configure two different pipes out of it.
fn pipe_request(properties: &Value, without: &[&str]) -> Map<String, Value> {
    let mut request = match properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    for member in without {
        request.remove(*member);
    }
    request
}

/// One answered pipe as the resource's ATTRIBUTES: the values the type publishes that are
/// EventBridge Pipes' own to report.
///
/// `AWS::Pipes::Pipe`'s read-only properties are its `Arn`, `CurrentState`, `StateReason`,
/// `CreationTime` and `LastModifiedTime` — each a member of that service's own answers — so
/// each is read off the answer under its own name, and one the answer does not carry
/// contributes nothing. The two instants are rendered as the ISO-8601 text a template value
/// is ([`instant_text`]), the `restJson1` wire carrying a timestamp as a number.
fn pipe_attributes(pipe: &Value) -> BTreeMap<String, String> {
    let mut attributes = reported_attributes(
        pipe,
        &[
            ("Arn", "Arn"),
            ("CurrentState", "CurrentState"),
            ("StateReason", "StateReason"),
        ],
    );
    for member in ["CreationTime", "LastModifiedTime"] {
        if let Some(instant) = pipe.get(member).and_then(instant_text) {
            attributes.insert(member.to_string(), instant);
        }
    }
    attributes
}
