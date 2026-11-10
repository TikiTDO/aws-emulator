Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resource-explorer-2.list-resources-without-view-or-default-succeeds
Issue-Shape-Id: com.amazonaws.resourceexplorer2#ResourceExplorer
Issue-Key: resource-explorer-2|ListResources|UnauthorizedException
Issue-Commit: 13cbee553af2e776db8b2ebed42254e169329488

# ListResources refuses with 401 UnauthorizedException when the Region has no default view, instead of succeeding with a Resources list

> _Note: this issue was identified against the code base at commit `13cbee553af2e776db8b2ebed42254e169329488`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResources

## Description
REOPENS 20261029T144855Z-resource-explorer-2-listresources-is-a-scaffold-stub-every-resource-explorer.md: its fix left the call refused. That issue's fix implemented the ListResources handler (the operation no longer answers 501), but the no-view path it describes was implemented as a refusal, so a ListResources call that AWS answers 200 is still refused by the emulator.

One root cause (Cause 1 of 1).

Cause 1 of 1: ListResources called with no ViewArn in a Region that has no default view is refused with HTTP 401 UnauthorizedException instead of succeeding.

AWS: Smithy com.amazonaws.resourceexplorer2#ListResources — POST /ListResources, success status 200 (defaulted), output ListResourcesOutput{Resources: ResourceList, NextToken, ViewArn}, @readonly, @paginated(items: Resources). ViewArn is an OPTIONAL input member (String @length 0..1000) — nothing in the model makes a view mandatory, and UnauthorizedException (httpError 401) is only one of the modeled refusals (AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, UnauthorizedException 401, ValidationException 400). The operation's prose documentation claims the call fails 401 when the Region has no default view, but recorded behavior only: with an ACTIVE index, no default view associated in the Region, and no ViewArn in the request, real AWS answers HTTP 200 with a ListResourcesOutput carrying a Resources list (possibly empty). The behavioral check encodes exactly that recorded divergence from the prose ("must succeed (200) with a Resources list, contrary to documentation claiming 401"). The modeled 401 remains correct only for a view the caller may not use — not for the absence of a default view.

Emulator: the same call is refused with HTTP 401, error code UnauthorizedException, message 'Unauthorized'. In that message 'Unauthorized' is the whole fixed text of this error class and has no variable part; the divergence is not the wording but that the error is raised at all — AWS returns 200/ListResourcesOutput here. Replay: botocore.errorfactory.UnauthorizedException: An error occurred (UnauthorizedException) when calling the ListResources operation: Unauthorized, raised on the first ListResources() call made with no arguments after DisassociateDefaultView, while the positive control ListResources(ViewArn=<view>) is expected to succeed and echo that ViewArn.

Why: a client enumerating account resources with the service's primary read path and no explicit view gets a hard authorization error where AWS returns results. Because UnauthorizedException is a client, non-retryable auth failure, SDK and application error handling classifies it as a permissions problem: callers log or surface a bogus authorization defect, abandon the enumeration, or retry credential acquisition, instead of consuming the (possibly empty) Resources page they would get from AWS. Any workflow that indexes resources and then lists them without first creating and associating a view cannot run at all against the emulator.

Fix: the ListResources handler's view resolution for the resource-explorer-2 service — services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/list_resources.rs. When ViewArn is absent and the Region has no associated default view, do not raise UnauthorizedException: resolve the query against the Region's indexed resources with no view filter and return 200 with Resources (and no ViewArn echoed, since no view was used), keeping the modeled 401 for a view the caller may not use. The explicit-ViewArn path must keep echoing the requested ViewArn.

Not covered by anything already open: 20261108T081111Z-resource-explorer-2-listresources-stub.md is keyed resource-explorer-2|ListResources|NotImplemented and describes the operation as an unimplemented scaffold answering 501 — the operation now answers 200/401, so that issue's described defect no longer reproduces and the replay report marked this failure neither already-filed nor owned. 20261110T193424Z-resource-explorer-2-listresources-response-bodies-diverge-from.md owns the ListResources timestamp/Properties.Data serialization and the 401 body's missing message member — it is about how a 401 is SERIALIZED, not about a 401 being raised where AWS returns 200. 20261108T081111Z-resource-explorer-2-associatedefaultview-refuses-a-deleted-view-with-the-wrong.md owns the other failing check in this run (delete-default-view-clears-default-pointer) and is deliberately not re-filed.

Traces: resource-explorer-2.list-resources-without-view-or-default-succeeds.

## Evidence
Failing check: resource-explorer-2.list-resources-without-view-or-default-succeeds (verdict: error).

Setup performed by the check before the diverging call: CreateIndex + poll GetIndex until State == 'ACTIVE'; GetDefaultView and, if a default existed, DisassociateDefaultView; CreateView(ViewName=<unique>, Tags={'sim2real:managed':'true'}); assert GetDefaultView returns a body with NO 'ViewArn' key (this assertion passes, so the Region genuinely has no default view).

Expected (recorded AWS): ListResources() with no arguments -> HTTP 200, body containing a 'Resources' list. Positive control ListResources(ViewArn=<created view>) -> HTTP 200 echoing ViewArn == that ARN.

Actual (emulator): ListResources() -> HTTP 401, Error.Code = 'UnauthorizedException', Error.Message = 'Unauthorized':
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.UnauthorizedException: An error occurred (UnauthorizedException) when calling the ListResources operation: Unauthorized
The check never reaches the positive control, so the explicit-ViewArn echo is ungraded in this run.

Message-part breakdown: 'Unauthorized' is wholly fixed text for the UnauthorizedException class and carries no request- or state-derived value; the divergence is the 401 status and UnauthorizedException code themselves, raised where the recording is 200 with ListResourcesOutput.

Model evidence (com.amazonaws.resourceexplorer2#ListResources): http POST /ListResources, code 200 (defaulted); input member ViewArn is optional (String, length 0..1000); output ListResourcesOutput{Resources: ResourceList, NextToken, ViewArn}; modeled errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), UnauthorizedException(401), ValidationException(400).

Generated view names/ARNs, index ARNs and timestamps in the replay output are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore client with service name 'resource-explorer-2' at it.
2. CreateIndex(Tags={'sim2real:managed':'true'}) and poll GetIndex until State == 'ACTIVE'.
3. Call GetDefaultView; if the body carries 'ViewArn', call DisassociateDefaultView so the Region has no default view.
4. CreateView(ViewName=<unique name>, Tags={'sim2real:managed':'true'}) (a view exists but is NOT associated as the default).
5. Confirm GetDefaultView returns 200 with no 'ViewArn' key.
6. Call ListResources() with no arguments.
   Observed: HTTP 401 UnauthorizedException, message 'Unauthorized'.
   Expected: HTTP 200 with a 'Resources' list in the body.
7. Call ListResources(ViewArn=<the view from step 4>) and confirm it still returns 200 and echoes that ViewArn.
8. Re-replay check resource-explorer-2.list-resources-without-view-or-default-succeeds to confirm.
