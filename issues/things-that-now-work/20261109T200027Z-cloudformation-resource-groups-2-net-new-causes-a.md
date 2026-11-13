Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__resource-groups.deploy-resourcegroups-group-tag-query-round-trip-and-delete cloudformation__resource-groups.deploy-resourcegroups-group-duplicate-name-rejected-leaving-the-original
Issue-Key: cloudformation__resource-groups.deploy-resourcegroups-group-tag-query-round-trip-and-delete|run#14|AssertionError|assert listed == [(out_name, out_arn)], (
Issue-Key: cloudformation__resource-groups|GetGroup|NotFoundException
Issue-Commit: 7e082a9dfef7650059a714e1b731386462df20dc

# cloudformation__resource-groups: 2 net-new causes — a stack-created AWS::ResourceGroups::Group is never registered with Resource Groups, and its GetAtt Arn uses the 'resourcegroups' namespace instead of 'resource-groups'

> _Note: this issue was identified against the code base at commit `7e082a9dfef7650059a714e1b731386462df20dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new causes behind the 2 failing cloudformation__resource-groups checks. Both stacks reach a terminal CloudFormation status and Ref/GetAtt resolve, so neither is a resource-type registry gap. NOTE: open issue 20261109T123922Z-application-insights-cloudformation-reso-application-insights-cloudformation-resource-groups-a-stack-created.md describes a similar stack-created-group-invisible symptom, but its replay keys are the application-insights__cloudformation__resource-groups checks only and do not carry either check listed here; these checks are therefore not covered by it.

Cause 1 of 2: an AWS::ResourceGroups::Group provisioned by a CREATE_COMPLETE stack does not exist in Resource Groups state.
AWS: recorded behavior only for the CloudFormation side — once a stack declaring AWS::ResourceGroups::Group reaches CREATE_COMPLETE, the group exists in Resource Groups exactly as if CreateGroup had been called with the template's Name/Description/ResourceQuery, so ListGroups returns one GroupIdentifiers entry whose GroupName is the declared Name and whose GroupArn equals the stack's Fn::GetAtt 'Arn' output, and GetGroup(Group=<Name>) returns that group (HTTP 200, Group.Name/GroupArn/Description as declared) rather than an error. Smithy (resource-groups): CreateGroup Name is @required with pattern ^[a-zA-Z0-9_\.-]+$; ListGroups output member GroupIdentifiers; GetGroup models NotFoundException (client, HTTP 404) only for a group that does not exist.
Emulator: after the stack reports CREATE_COMPLETE and resolves Ref/GetAtt for the group, Resource Groups does not know it. ListGroups returns [] filtered to the declared name, and GetGroup(Group=<declared name>) answers HTTP 404 NotFoundException with message "The specified group does not exist" (fixed text; AWS emits that text only when no such group exists). The create path of the CloudFormation resource handler therefore reports success without writing resource-groups state.
Why: a client cannot read, tag, query or delete a resource group its template just created; anything that resolves group membership (GetGroupQuery, ListGroupResources, SearchResources, Application Insights applications bound to the group) has nothing to resolve, and the duplicate-name path cannot be exercised at all because the first group never exists — the whole CFN->Resource Groups integration is inert while the stack claims success.
Fix: the AWS::ResourceGroups::Group resource handler's create path in the CloudFormation resource-provisioning layer — it must perform the resource-groups CreateGroup (name, description, resource query, propagated stack tags) against resource-groups state, and its delete path must remove it so a post-DeleteStack GetGroup is refused with NotFoundException.
Traces: cloudformation__resource-groups.deploy-resourcegroups-group-tag-query-round-trip-and-delete, cloudformation__resource-groups.deploy-resourcegroups-group-duplicate-name-rejected-leaving-the-original

Cause 2 of 2: the group ARN handed back by Fn::GetAtt 'Arn' spells the ARN service namespace 'resourcegroups' instead of 'resource-groups'.
AWS: Smithy (resource-groups) GetGroup input member Group targets GroupStringV2 with pattern ^[a-zA-Z0-9_\.-]{1,300}|[a-zA-Z0-9_\.-]{1,150}/[a-z0-9]{26}|arn:aws(-[a-z]+)*:resource-groups:[a-z]{2}(-[a-z]+)+-\d{1}:[0-9]{12}:group/(...)$ — the ARN namespace for a group is the hyphenated 'resource-groups', so a group ARN has the fixed shape arn:<partition>:resource-groups:<region>:<account-id>:group/<group-name> with partition, region, account id and group name taken from the request and the serving environment.
Emulator: the stack's Fn::GetAtt 'Arn' output (and the ARN the handler mints for the group) uses the unhyphenated namespace, i.e. arn:aws:resourcegroups:<region>:<account-id>:group/<group-name>.
Why: that string does not match the modeled GroupStringV2 pattern, so a client that round-trips the template output into GetGroup/GetGroupQuery/Tag/Untag by ARN is refused client-side by the SDK or server-side as a bad request, and any ARN comparison against a real-AWS-shaped ARN fails.
Fix: the ARN construction for AWS::ResourceGroups::Group in the resource handler / the resource-groups crate's ARN builder — emit the arnNamespace 'resource-groups'.
Traces: cloudformation__resource-groups.deploy-resourcegroups-group-tag-query-round-trip-and-delete (the failure message quotes the GetAtt Arn it produced).

## Evidence
Cause 1:
- cloudformation__resource-groups.deploy-resourcegroups-group-tag-query-round-trip-and-delete — stack reaches CREATE_COMPLETE, Outputs resolve (Ref -> declared group name, GetAtt Arn -> a group ARN), then: AssertionError: ListGroups lists [] under '<declared-group-name>'; Ref/GetAtt are ('<declared-group-name>', '<group-arn>'). Expected exactly one ListGroups GroupIdentifiers entry equal to (Ref, GetAtt Arn).
- cloudformation__resource-groups.deploy-resourcegroups-group-duplicate-name-rejected-leaving-the-original — stack A reaches CREATE_COMPLETE, then botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the GetGroup operation: The specified group does not exist. Expected HTTP 200 with Group.Name/GroupArn/Description as declared in the template.

Cause 2:
- Same first check: the Arn handed back for the group is of the form arn:aws:resourcegroups:<region>:<account-id>:group/<group-name> (unhyphenated namespace). Expected arn:aws:resource-groups:<region>:<account-id>:group/<group-name>, the only group-ARN form the modeled GetGroup GroupStringV2 pattern accepts. (Region/account/name above are per-environment values, not requirements; the namespace token is.)

## Steps to Reproduce
1. CreateStack with a template whose only resource is AWS::ResourceGroups::Group {Name: <unique-name>, Description: <text>, ResourceQuery: {Type: TAG_FILTERS_1_0, Query: {ResourceTypeFilters: [AWS::AllSupported], TagFilters: [{Key: sim2real, Values: [<tag-value>]}]}}} and Outputs GroupName: {Ref: Group}, GroupArn: {Fn::GetAtt: [Group, Arn]}.
2. Poll DescribeStacks until CREATE_COMPLETE; read the Outputs.
3. Call resource-groups ListGroups (paginated) and filter GroupIdentifiers by the declared name: observe [] instead of one entry matching (Ref, GetAtt Arn). — Cause 1.
4. Call resource-groups GetGroup(Group=<declared name>): observe HTTP 404 NotFoundException "The specified group does not exist" instead of the group. — Cause 1.
5. Inspect the GroupArn output: observe the 'resourcegroups' ARN namespace instead of 'resource-groups'. — Cause 2.
6. Re-replay both checks in trace_ids.
