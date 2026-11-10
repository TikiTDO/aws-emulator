Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot|_http#45|Timeout|with urllib.request.urlopen(req, timeout=timeout_s) as resp:
Issue-Commit: 8c5ca24b773ca2762f91097a2e2db632761ff691

# apigatewayv2: a non-autoDeploy stage pinned to a DeploymentId reaches no verdict — the manual-stage deployment-snapshot scenario hangs until the harness kills it at 60s

> _Note: this issue was identified against the code base at commit `8c5ca24b773ca2762f91097a2e2db632761ff691`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
ONE net-new root cause on the apigatewayv2 service token, taken from the single failure of this replay that the report marks neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: the stage/deployment snapshot scenario produces no response the check can grade — both stage URLs never serve their integration inside the harness budget, so the check is killed at 60 s ===

AWS: the control-plane wiring is modelled. Smithy CreateStage (POST /v2/apis/{ApiId}/stages, 201) takes both `AutoDeploy` (jsonName `autoDeploy`, boolean) and `DeploymentId` (jsonName `deploymentId`); Smithy CreateDeployment (POST /v2/apis/{ApiId}/deployments, 201) takes an optional `StageName` (jsonName `stageName`) and returns `DeploymentId`/`AutoDeployed`/`DeploymentStatus`; Smithy GetStage (GET /v2/apis/{ApiId}/stages/{StageName}, 200) returns `AutoDeploy` and `DeploymentId`. The data-plane payload itself is not modelled — recorded behavior only. Recorded: a stage created with an explicit `DeploymentId` and without AutoDeploy keeps serving THAT deployment's snapshot; mutating the API afterwards (UpdateIntegration PATCH /v2/apis/{ApiId}/integrations/{IntegrationId}, changing IntegrationUri from backend A to backend B) changes nothing that stage serves, while the AutoDeploy=true `$default` twin converges on backend B and GetStage($default) reports a NEW deploymentId. The pinned stage only moves after an explicit CreateDeployment(StageName=<pinned>), after which GetStage(<pinned>).DeploymentId equals the newly returned deploymentId and the stage serves backend B. Both stage URLs answer promptly with HTTP 200 and the integration's body throughout (https://{api-id}.execute-api.{region}.amazonaws.com/snap for $default, /{stage}/snap for the named stage). Reference: https://docs.aws.amazon.com/apigateway/latest/developerguide/http-api-stages.html

Emulator: the check never reaches ANY graded assertion — the replay reports `Error: Check timed out after 60.0s`. The first gate is a stability wait that requires both the `$default` (AutoDeploy) URL and the pinned stage URL to answer (200, <backend-A marker>) continuously; against AWS that settles in seconds and the whole scenario (a 15 s hold window at minimum, then one CreateDeployment) fits inside the budget. Here the poll loops run to the harness cap, i.e. the emulator serves neither stage URL the expected 200+body (or does not answer at all) and no terminal result is produced. Nothing downstream — that a pinned stage holds its snapshot after UpdateIntegration, that AutoDeploy mints a new deploymentId, that CreateDeployment(StageName=…) re-points the pinned stage — is ever exercised. Related open issue 20261107T125951Z-apigatewayv2-execute-api-data-plane-7-net-new.md documents other HTTP data-plane defects (stage segment injected into rawPath, 403 where AWS answers 404) and 20261107T125951Z-apigatewayv2-7-net-new-root-causes-where-the.md covers further stage-serving defects; the replay marks this check as owned by NEITHER, so it is filed here and should be cross-checked against them.

Why: a client gets no answer at all on the execute-api host for a deployed stage — a hang cannot be caught, retried or classified, unlike any status AWS would return. And the whole point of a non-autoDeploy stage — pinning production to a known deployment so control-plane edits cannot change what it serves — is unverifiable: code that relies on stage pinning, or on autoDeploy publishing a change, behaves arbitrarily against this emulator.

Fix: the apigatewayv2 execute-api HTTP data plane's stage→deployment resolution together with the CreateStage/CreateDeployment state that holds a per-stage deployment snapshot: a request to a named stage must be served from the deployment that stage is bound to, an AutoDeploy stage must mint and bind a new deployment on an API mutation, CreateDeployment(StageName=…) must re-bind the named stage, and every request that cannot be served must still get a terminal HTTP status instead of hanging.

Traces: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot.

## Evidence
Suite: simparity-behavioral-aws-v1. Service heading `apigatewayv2`: 127/173 (73%), 46 failed; 45 of those are marked ALREADY FILED or OWNED, this is the only survivor.

[mismatch] apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot
  Error: Check timed out after 60.0s

Expected (AWS): GET on the $default URL and on the pinned stage URL both answer 200 with backend A's body continuously; after UpdateIntegration(IntegrationUri=backend B) the $default twin converges on backend B and GetStage($default).DeploymentId changes, while the pinned stage returns backend A for the whole hold window (>= 5 samples) and GetStage(pinned).DeploymentId is unchanged; then CreateDeployment(ApiId, StageName='pinned') returns a new deploymentId, the pinned URL converges on backend B, and GetStage(pinned).DeploymentId equals that new id.
Actual (emulator): no verdict — the check consumes the entire 60 s harness budget in its initial wait for both stage URLs to serve (200, backend-A body), so not one of the assertions above is reached. Marker names, api ids and deployment ids in the check are per-run values; the requirement is the status/body/DeploymentId relationships, not those literals.

## Steps to Reproduce
1. Create two Lambda functions A and B behind the same handler, differing only in a MARKER environment variable returned as the 200 text/plain body.
2. CreateApi(Name=…, ProtocolType='HTTP'); CreateIntegration(IntegrationType='AWS_PROXY', IntegrationUri=<A arn>, IntegrationMethod='POST', PayloadFormatVersion='2.0'); CreateRoute(RouteKey='GET /snap', Target='integrations/<id>'); grant lambda:InvokeFunction to apigateway.amazonaws.com for both functions with SourceArn arn:aws:execute-api:{region}:{account}:{api-id}/*/*.
3. CreateStage(StageName='$default', AutoDeploy=true); CreateDeployment(ApiId) -> d1; CreateStage(StageName='pinned', DeploymentId=d1).
4. GET https://{api-id}.execute-api.{region}.amazonaws.com/snap and https://{api-id}.execute-api.{region}.amazonaws.com/pinned/snap. Both must answer 200 with A's marker promptly and repeatedly. The emulator never gets past this step inside 60 s.
5. UpdateIntegration(ApiId, IntegrationId, IntegrationUri=<B arn>). Poll both URLs: $default must converge on B's marker and GetStage($default).DeploymentId must change; the pinned URL must keep returning A's marker for a hold window of >= 5 samples and GetStage('pinned').DeploymentId must stay d1.
6. CreateDeployment(ApiId, StageName='pinned') -> d2 (!= d1). The pinned URL must converge on B's marker and GetStage('pinned').DeploymentId must equal d2.
7. After fixing, re-replay the id in trace_ids.
