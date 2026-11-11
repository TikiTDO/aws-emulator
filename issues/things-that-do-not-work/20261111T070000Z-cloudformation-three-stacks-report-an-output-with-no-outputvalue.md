Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cfn-cidr-computed-blocks-accepted-by-ec2-and-bad-count-rejected cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance
Issue-Key: cloudformation__ec2.cfn-cidr-computed-blocks-accepted-by-ec2-and-bad-count-rejected|run#10|KeyError|out = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set|_evaluate#10|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in described.get("Outputs") or []}
Issue-Key: cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance|_outputs#13|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in _stack(cfn, stack_id).get("Outputs", [])}
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# CloudFormation: three stacks still report an output with no OutputValue, so the deploy-then-use pattern stays dead for computed and zone-derived values

> _Note: this cause was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Provenance: this was cause 1 of 5 of `20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md`. Cause 3 of that issue (a CFN-created resource's physical id must be the identifier the owning service issued) is fixed and that report now sits under `issues/things-that-now-work/`; this cause is unfixed and is re-filed here so its three remaining checks are not recorded as working. Their three `Issue-Key` lines and three `Issue-Trace-Ids` moved with it. The fourth check of the original cause, `cloudformation__ec2__sqs__ssm.cfn-yaml-short-form-intrinsics-shape-resources`, passes now and is not carried here. Open issue `20261022T202249Z-autoscaling-cloudformation-ec2-cloudformation-describestacks-returns-stack-outputs-entries-without-outputvalue.md` describes the same symptom for the autoscaling-led checks and owns those._

## Operation
DescribeStacks (the stack-output evaluation behind it)

## Description
AWS: `DescribeStacks` on a `CREATE_COMPLETE` stack returns `Stacks[].Outputs[]` where each `Output` carries its `OutputKey` and the RESOLVED `OutputValue` (Smithy com-amazonaws-cloudformation-cloudformation: `Output` members `OutputKey`/`OutputValue`/`Description`/`ExportName`).

Emulator: three checks still raise `KeyError: 'OutputValue'` — the `Output` entry exists and the value member is absent, which is what `crate::state::output_views` reports for a declared output whose value this service could not evaluate (`StoredOutput::value` is `None`).

Why: any client that deploys a stack and then reads an output to drive the next call cannot proceed at all.

What is known about each of the three, for whoever picks this up:

* `cfn-cidr-computed-blocks-accepted-by-ec2-and-bad-count-rejected` — the failure is the FIRST read of the stack's outputs (`run#10`). The expressions the suite `services/service-com-amazonaws-cloudformation-cloudformation/tests/all/stack_output_resolution.rs` already pins — `Fn::Cidr` in both address families, `Fn::Select`, `Fn::Join`, `Fn::Length`, and `Fn::GetAtt` on a VPC's and a subnet's `CidrBlock` — all resolve, so the template's unresolved expression is not one of those; it has to be identified before a rule can be stated. The check's second half ("bad count rejected") wants a template whose `Fn::Cidr` cannot be satisfied to be REFUSED, where `crate::state::template::cidr` resolves it to no value instead.
* `getazs-resolves-to-and-tracks-the-default-subnet-set` — this one has a second, independent divergence past the output read: `Fn::GetAZs` must resolve to the zones the account has a DEFAULT SUBNET in and must track a change to that set (`expected ['us-west-2a', 'us-west-2b'], got ['us-west-2a', 'us-west-2b', 'us-west-2c']` after the default subnet in `us-west-2c` is removed). `crate::state::template::availability_zones` derives the list from the Region name alone (`AVAILABILITY_ZONES_PER_REGION`), so it cannot track Amazon EC2's `default-for-az` subnets; threading the account's default-subnet zones into `template::Scope` (the way `stack_outputs::Values` is threaded) is the shape of that fix.
* `deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance` — past the output read this check waits for an EC2 instance to be reported `Online` by Systems Manager, which no component does yet; that half is the same missing capability as `20261111T070000Z-ssm-a-securestring-is-not-readable-as-ciphertext-and-no-ec2-instance-is-a-managed-node.md`.

## Evidence
Expected: `Outputs[].OutputValue` present on a `CREATE_COMPLETE` stack.

```
KeyError: 'OutputValue'
```

and, for the `Fn::GetAZs` check, past that point:

```
AssertionError: a subtractive change to the default subnet in 'us-west-2c' must move Fn::GetAZs:
expected ['us-west-2a', 'us-west-2b'], got ['us-west-2a', 'us-west-2b', 'us-west-2c']
```

## Steps to Reproduce
Replay the behavioral suite and re-run the three check ids in `Issue-Trace-Ids`. Minimal repros: (1) `CreateStack` with a template whose outputs are computed from `Fn::Cidr` over a VPC's CIDR, wait for `CREATE_COMPLETE`, `DescribeStacks` and read `Stacks[0].Outputs[0].OutputValue`; (2) deploy a stack whose output joins `Fn::GetAZs ""`, delete the default subnet of one zone in Amazon EC2, deploy again and read the output.
