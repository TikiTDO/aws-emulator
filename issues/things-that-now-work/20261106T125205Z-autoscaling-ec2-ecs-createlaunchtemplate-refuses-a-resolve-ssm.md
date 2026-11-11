Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__ecs.ecsnrt-capacity-provider-delete-while-associated-blocked autoscaling__ec2__ecs.ecsnrt-capacity-provider-name-and-group-reuse-rejected autoscaling__ec2__ecs.ecsnrt-capacity-provider-service-on-zero-capacity-group-parks-task-provisioning autoscaling__ec2__ecs.ecsnrt-cluster-default-strategy-decides-service-launch-type autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it
Issue-Key: ec2|CreateLaunchTemplate|SsmInvalidParameter
Issue-Commit: 9cff6bb637b3e82ef8d39ccec86044fadd540cad

# autoscaling__ec2__ecs: CreateLaunchTemplate refuses a resolve:ssm: ImageId naming an AWS-published public parameter (SsmInvalidParameter), so the whole ECS capacity-provider scenario family cannot start

> _Note: this issue was identified against the code base at commit `9cff6bb637b3e82ef8d39ccec86044fadd540cad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLaunchTemplate

## Description
Cause 1 of 1: EC2 CreateLaunchTemplate rejects a LaunchTemplateData.ImageId given in the documented `resolve:ssm:<public-parameter>` alias form, instead of accepting it and deferring resolution to launch time. All five failing checks are the same call with the same alias.

AWS: Smithy (com.amazonaws.ec2) CreateLaunchTemplate, input LaunchTemplateData of shape RequestLaunchTemplateData, member ImageId. Its modelled documentation states: "The ID of the AMI in the format ami-0ac394d6a3example. Alternatively, you can specify a Systems Manager parameter, using one of the following formats. The Systems Manager parameter will resolve to an AMI ID on launch" — and lists `resolve:ssm:<public-parameter>` first among the accepted forms. `/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id` is an AWS-published read-only public parameter present in every commercial region, so `ImageId="resolve:ssm:/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id"` is a valid value: AWS answers HTTP 200 with the LaunchTemplate element (LaunchTemplateId/LaunchTemplateName/DefaultVersionNumber/LatestVersionNumber) and stores the alias string verbatim in the version's ImageId; the AMI id is only resolved when an instance is launched from the template. There is no refusal for this input.

Emulator: the same call is refused. Replay records `botocore.exceptions.ClientError: An error occurred (SsmInvalidParameter) when calling the CreateLaunchTemplate operation: The following aliases are invalid: /aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id` (a non-2xx error response where AWS answers 200). In that message the fixed text is `The following aliases are invalid: ` and the variable part is the alias path taken verbatim from the request's ImageId (minus the `resolve:ssm:` prefix); it is emulator-authored for an input AWS does not refuse at all. The emulator evidently resolves the alias eagerly at CreateLaunchTemplate time against its own parameter store, and that store does not carry the AWS-published `/aws/service/...` namespace — the same missing namespace already filed on the SSM side as open issue 20261103T154947Z-ssm-getparameter-cannot-resolve-aws-published-public-aws.md (GetParameter on `/aws/service/ami-amazon-linux-latest/...`). That issue does not cover this one: this is EC2's CreateLaunchTemplate validation path refusing a request, with a different operation, error code and message, and it fails even for a caller that never calls SSM.

Why: a client that follows the documented launch-template idiom — pin the ECS-optimized AMI by SSM alias rather than a hard-coded ami- id — cannot create a launch template at all. Because the refusal lands on the very first resource every ECS capacity-provider scenario builds (launch template -> zero-capacity Auto Scaling group -> ECS capacity provider -> cluster association), nothing downstream is reachable: capacity-provider creation, PutClusterCapacityProviders, DeleteCapacityProvider's DELETE_FAILED/ACTIVE behaviour, cluster default-strategy launch-type selection and RunTask parking are all unobservable. A client also cannot tell a genuinely bad alias from an emulator with no published parameter namespace.

Fix: the EC2 CreateLaunchTemplate handler and its ImageId validation in the ec2 service crate — accept the `resolve:ssm:` forms for ImageId (public parameter, parameter-name[:version|:label], parameter-ARN[:version|:label]), store the alias verbatim in the launch-template version rather than resolving it during create, and resolve it only at instance launch. Where an alias genuinely cannot be resolved, that refusal belongs on the launch path, not on CreateLaunchTemplate. The underlying gap — the AWS-published `/aws/service/...` public-parameter namespace being absent from the emulator's SSM parameter state — must be filled for the `/aws/service/ecs/optimized-ami/...` hierarchy as well, so a launch from such a template can resolve.

Traces: the five autoscaling__ec2__ecs `ecsnrt-*` checks listed in trace_ids, each failing on the same CreateLaunchTemplate call in their shared `_capacity_group` helper.

## Evidence
Behavioral suite, service heading `### autoscaling__ec2__ecs`. All five failing checks abort at the same call, in the shared helper `_capacity_group(ctx, subnet)`:

    AMI_ALIAS = "resolve:ssm:/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id"
    ec2.create_launch_template(
        LaunchTemplateName=ctx.unique_name("sim2real-ecsnrt-lt-"),
        LaunchTemplateData={"ImageId": AMI_ALIAS, "InstanceType": "t3.micro"},
        TagSpecifications=[{"ResourceType": "launch-template", "Tags": [{"Key": "sim2real:managed", "Value": "true"}]}],
    )

Expected (AWS): HTTP 200, a LaunchTemplate element whose version holds ImageId exactly as submitted (the alias string); the alias resolves to an AMI id only when an instance is launched.

Actual (emulator), identical for all five checks:

    botocore.exceptions.ClientError: An error occurred (SsmInvalidParameter) when calling the CreateLaunchTemplate operation: The following aliases are invalid: /aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id

Message anatomy: fixed text `The following aliases are invalid: `; variable part the alias path echoed from the request. No launch template is created, so every assertion in the five checks (DeleteCapacityProvider -> (ACTIVE, DELETE_FAILED), the cluster keeping the provider, name/group reuse refusal, default-strategy launch type, RunTask parking) is never reached. The generated launch-template/ASG/provider names and ids in these runs are per-run values and are not part of the requirement; the requirement is that the call succeeds with 200.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Call EC2 CreateLaunchTemplate with LaunchTemplateName=<any unused name> and LaunchTemplateData={"ImageId": "resolve:ssm:/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id", "InstanceType": "t3.micro"}.
3. Observe HTTP 400 with error code SsmInvalidParameter and message "The following aliases are invalid: /aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id"; real AWS answers 200 with the LaunchTemplate element and stores the alias unresolved.
4. Re-replay the five behavioral checks in trace_ids (service autoscaling__ec2__ecs): each aborts at this call inside its `_capacity_group` helper before any ECS capacity-provider assertion runs.
