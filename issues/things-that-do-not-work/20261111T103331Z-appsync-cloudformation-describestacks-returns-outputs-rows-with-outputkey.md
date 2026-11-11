Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cloudformation.cfn-appsync-api-key-in-place-update-retained-replacement-then-not-found appsync__cloudformation.cfn-appsync-data-source-in-place-update-retained-replacement-then-not-found appsync__cloudformation.cfn-appsync-graphql-api-in-place-update-serves-query-then-not-found appsync__cloudformation.cfn-appsync-graphql-schema-in-place-update-serves-new-fields-then-not-found appsync__cloudformation.cfn-appsync-resolver-in-place-update-retained-replacement-then-not-found
Issue-Key: appsync__cloudformation.cfn-appsync-api-key-in-place-update-retained-replacement-then-not-found|_outputs#2|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: appsync__cloudformation.cfn-appsync-data-source-in-place-update-retained-replacement-then-not-found|_outputs#2|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: appsync__cloudformation.cfn-appsync-graphql-api-in-place-update-serves-query-then-not-found|_outputs#2|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: appsync__cloudformation.cfn-appsync-graphql-schema-in-place-update-serves-new-fields-then-not-found|_outputs#2|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: appsync__cloudformation.cfn-appsync-resolver-in-place-update-retained-replacement-then-not-found|_outputs#2|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# appsync__cloudformation: DescribeStacks returns Outputs rows with OutputKey but no OutputValue, so the five AWS::AppSync::* in-place/replacement update checks cannot read any stack output

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One root cause, grounded in 5 failing behavioral checks of the `appsync__cloudformation` suite. All five deploy a CloudFormation template whose Outputs resolve AppSync resource attributes (`Fn::GetAtt` on AWS::AppSync::GraphQLApi `ApiId`/`GraphQLUrl`, on AWS::AppSync::ApiKey `ApiKeyId`/`ApiKey`, `Ref` on the key/resolver/data-source), wait for CREATE_COMPLETE, then read `Stacks[0].Outputs`. All five die on the FIRST read of the outputs map, before any AppSync in-place-update or UpdateReplacePolicy assertion is ever reached.

Cause 1 of 1: DescribeStacks on a CREATE_COMPLETE stack emits Outputs entries that carry OutputKey and no OutputValue.

AWS: Smithy (cloudformation) `Output` structure has members `OutputKey` ("The key associated with the output"), `OutputValue` ("The value associated with the output"), `Description`, `ExportName`; `Stack.Outputs` is a list of `Output`. For a stack whose StackStatus is CREATE_COMPLETE (or UPDATE_COMPLETE), real CloudFormation returns every template Output with BOTH `OutputKey` and `OutputValue` present, with the intrinsic function already resolved to a concrete value; a resource attribute that cannot be resolved fails the resource and the stack instead of yielding a half-populated Output row. DescribeStacks returns HTTP 200 in both cases, so there is no error code to compare — the divergence is purely in the response shape.
Emulator: the stack does reach a terminal `*_COMPLETE` status (the checks' `_wait_stack`/`_expect` helpers assert the status first and passed it), but at least one entry of the returned `Outputs` list has no `OutputValue` member at all, so the standard client idiom `{o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}` raises `KeyError: 'OutputValue'` in every one of the five checks (`_outputs()` helper, reached from `run()` right after the create wait).
Why: a client cannot retrieve ANY value a stack published — the API id, the GraphQL URL, the API key value, the resolver/data-source ARN. Everything downstream is unreachable: the data-plane query against the published GraphQL URL, the GetGraphqlApi/GetDataSource/GetResolver reads, the in-place-update assertions and the post-delete NotFoundException assertions. Worse than an outright failure: the stack reports success, so the caller believes the deploy produced resolvable attributes and only discovers otherwise when it dereferences an output. Any IaC-driven workflow (CDK, `cloudformation deploy`, a shell script piping `--query 'Stacks[0].Outputs'`) breaks at this point.
Fix: the CloudFormation stack output-resolution step plus the DescribeStacks response serializer — the intrinsic (`Ref`/`Fn::GetAtt`) behind each template Output must be resolved during create/update for the AWS::AppSync::GraphQLApi, GraphQLSchema, ApiKey, DataSource and Resolver handlers' attributes, and the serializer must never emit an `Output` member without its `OutputValue`; an attribute that cannot be resolved must fail the resource (CREATE_FAILED) instead of producing a valueless Output row.
Traces: appsync__cloudformation.cfn-appsync-api-key-in-place-update-retained-replacement-then-not-found, appsync__cloudformation.cfn-appsync-data-source-in-place-update-retained-replacement-then-not-found, appsync__cloudformation.cfn-appsync-graphql-api-in-place-update-serves-query-then-not-found, appsync__cloudformation.cfn-appsync-graphql-schema-in-place-update-serves-new-fields-then-not-found, appsync__cloudformation.cfn-appsync-resolver-in-place-update-retained-replacement-then-not-found.

Related but NOT the same work item: open issue 20261024T032443Z-appsync-cloudformation-deploys-of-appsync-templates-diverge-valueless-stack.md describes the same class of defect (its Cause 1) for a different set of AppSync deploy checks, and 20261110T220525Z-cloudformation-cloudwatch-ecr-ecs-elbv2-logs-a-nested.md / 20261110T124641Z-cloudformation-a-nested-stack-s-parent-returns-outputs.md report it for nested stacks. None of them carries the replay keys of the five checks above, so those five are still unverified; a fix must be re-replayed against the trace_ids listed here.

## Evidence
Expected (real AWS): DescribeStacks on the CREATE_COMPLETE stack returns, for each template Output, an entry with both members, e.g. Stacks[0].Outputs = [{"OutputKey": "ApiA", "OutputValue": "<api id>"}, {"OutputKey": "UrlA", "OutputValue": "<graphql url>"}, {"OutputKey": "KeyValue", "OutputValue": "<api key>"}, ...] — keys are the template's Output names, values are whatever the run's resources resolved to.

Actual (emulator): at least one entry has only OutputKey, so the checks fail identically on the dict comprehension:

  File "check_appsync__cloudformation_cfn_appsync_api_key_in_place_update_retained_replacement_then_not_found.py", line 123, in run
      out = _outputs(stack)
  File "...", line 349, in _outputs
      return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
  KeyError: 'OutputValue'

Identical trace, same `_outputs()` helper, in all five checks:
- cfn-appsync-api-key-in-place-update-retained-replacement-then-not-found (run line 123 -> _outputs line 349)
- cfn-appsync-data-source-in-place-update-retained-replacement-then-not-found (run line 134 -> _outputs line 360)
- cfn-appsync-graphql-api-in-place-update-serves-query-then-not-found (run line 124 -> _outputs line 314)
- cfn-appsync-graphql-schema-in-place-update-serves-new-fields-then-not-found (run line 106 -> _outputs line 292)
- cfn-appsync-resolver-in-place-update-retained-replacement-then-not-found (run line 136 -> _outputs line 379)

Because the failure is on the first outputs read, nothing these checks were written to verify was exercised at all: the in-place Description/Name/XrayEnabled/Definition/Code updates, the UpdateReplacePolicy Retain behavior, the live GraphQL query against the published URL, and the post-stack-delete refusals (NotFoundException / "API not found." for GetGraphqlApi, GetDataSource, GetIntrospectionSchema; NotFoundException / "No resolver found." for a field with no resolver) are all still unverified.

## Steps to Reproduce
1. CreateStack with a template declaring AWS::AppSync::GraphQLApi (AuthenticationType API_KEY) plus AWS::AppSync::GraphQLSchema, and Outputs such as:
   {"Api": {"Value": {"Fn::GetAtt": ["ApiA", "ApiId"]}}, "Url": {"Value": {"Fn::GetAtt": ["ApiA", "GraphQLUrl"]}}}
   (the five checks additionally template AWS::AppSync::ApiKey with Outputs Ref/ApiKeyId/ApiKey, AWS::AppSync::DataSource of type NONE, and AWS::AppSync::Resolver with APPSYNC_JS Code).
2. Poll DescribeStacks until StackStatus no longer ends in _IN_PROGRESS; observe it is CREATE_COMPLETE.
3. Read Stacks[0].Outputs and build {o["OutputKey"]: o["OutputValue"] for o in Outputs} — this raises KeyError: 'OutputValue' because at least one row has no OutputValue member.
4. Equivalently from the CLI: `aws cloudformation describe-stacks --stack-name <id> --query 'Stacks[0].Outputs'` shows rows with OutputKey and no OutputValue.
