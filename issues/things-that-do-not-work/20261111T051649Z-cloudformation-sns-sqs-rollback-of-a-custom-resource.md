Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails
Issue-Key: cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails|run#19|AssertionError|assert early == [], f"the rollback asked to delete Fits before Overflows was deleted: {early!r}"
Issue-Commit: 950d97075a2dc28c97155d1939158688cbb186aa

# cloudformation__sns__sqs: rollback of a custom resource that failed on an oversized provider response deletes its DependsOn dependency before (or alongside) the failed resource itself

> _Note: this issue was identified against the code base at commit `950d97075a2dc28c97155d1939158688cbb186aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in the single surviving failing check of this service that the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: the rollback driven by a failed Custom:: resource publishes the provider Delete for the resource the failed one DependsOn before the failed resource's own Delete has been issued, so the rollback does not run in reverse dependency order.

AWS: recorded behavior only, against the documented custom-resource response protocol (CloudFormation User Guide, crpg-ref), plus the model's state vocabulary (Smithy: com.amazonaws.cloudformation#ResourceStatus includes CREATE_FAILED and DELETE_IN_PROGRESS; com.amazonaws.cloudformation#StackStatus includes ROLLBACK_IN_PROGRESS / ROLLBACK_COMPLETE; CreateStack / DescribeStackEvents / DescribeStackResource carry them). A provider response document whose serialized body exceeds 4,096 bytes is one CloudFormation cannot read a PhysicalResourceId out of, so it fails that resource (ResourceStatusReason exactly the fixed text `Response object is too long.`) and rolls the stack back. CloudFormation then drives the rollback itself, in reverse dependency order: it first asks the provider to Delete the FAILED resource — carrying a CloudFormation-assigned physical id of the form `<StackName>-<LogicalId>-<SUFFIX>`, where the stack name and logical id come from the request and the suffix is generated — and only after that Delete is published does it ask the provider to Delete any resource the failed one `DependsOn`. At the moment the failed resource's Delete request is delivered, no Delete for its dependency may be outstanding.

Emulator: the 4,096-byte bound is now enforced and a rollback does start (an improvement over the behaviour recorded in the open issue 20261106T014853Z-cloudformation-sns-sqs-a-custom-resource-provider-response.md, whose replay key no longer matches this check — the check now gets past the acceptance of the oversized response), but the Delete requests are published out of order: when the Delete for the failed resource `Overflows` is picked up, a Delete for its dependency `Fits` is ALREADY pending on the provider queue. The check fails with `AssertionError: the rollback asked to delete Fits before Overflows was deleted: ['Fits']`. Hypothesis: the rollback enumerates the stack's resources without honouring the DependsOn edge (or emits all Deletes in one pass) instead of serialising them in reverse dependency order and waiting for each provider verdict.

Why: a provider that implements the normal contract — handle the Delete for a resource only after the resources that depend on it are gone — is asked to tear down a dependency while the dependent resource still exists. Real providers refuse or corrupt state in that situation, so rollbacks that work against AWS fail or leak resources against the emulator, and a client cannot rely on the ordering guarantee CloudFormation gives.

Fix: the CloudFormation provisioning/rollback driver — the component that schedules provider requests during ROLLBACK_IN_PROGRESS must walk the resource dependency graph in reverse (the failed resource first, then what it DependsOn) and must not publish a dependency's Delete until the dependent resource's Delete has been issued and answered.

Traces: cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails.

## Evidence
Cause 1 — cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails [mismatch].

Detail from the replay report:
```
File "check_cloudformation__sns__sqs_custom_resource_response_over_4096_bytes_fails.py", line 78, in run
assert early == [], f"the rollback asked to delete Fits before Overflows was deleted: {early!r}"
AssertionError: the rollback asked to delete Fits before Overflows was deleted: ['Fits']
```

Expected: after a 4,097-byte provider response fails `Overflows` (which carries `DependsOn: Fits`), the next provider request for that stack is a Delete for `Overflows` with a PhysicalResourceId matching `<StackName>-Overflows-[A-Z0-9]+`, and at that moment NO Delete request for `Fits` is pending; the Delete for `Fits` is published only afterwards, carrying the provider-supplied physical id from its own Create.

Actual: at the moment the `Overflows` Delete is read off the subscribed queue, a Delete request for `Fits` is already pending on that queue (`['Fits']`), i.e. the dependency's teardown was published before/alongside the failed resource's teardown.

(The stack name, logical-id suffix and generated ids in any single run are per-run values; the requirement is the ordering — failed resource's Delete before its DependsOn target's Delete — and the `<StackName>-<LogicalId>-<SUFFIX>` shape of the CloudFormation-assigned id.)

## Steps to Reproduce
Setup (the provider): create an SNS topic; create an SQS queue; set the queue's `Policy` to allow `sns.amazonaws.com` `sqs:SendMessage` on the queue ARN with `Condition.ArnEquals.aws:SourceArn` = the topic ARN; `Subscribe` the queue ARN with Protocol=sqs and Attributes `{"RawMessageDelivery": "true"}`. Each custom-resource request then arrives as a raw queue message and is answered by PUTting a response document at the request's presigned `ResponseURL`.

1. `CreateStack` with Resources: `Fits` = `Custom::Sized {ServiceToken: <topic arn>, ServiceTimeout: "120"}` and `Overflows` = `Custom::Sized {ServiceToken: <topic arn>, ServiceTimeout: "120"}` with `DependsOn: Fits`.
2. Read the `Fits` Create request off the queue and PUT a SUCCESS response whose serialized body is padded to exactly 4096 bytes. Poll `DescribeStackResource` until `Fits` is CREATE_COMPLETE.
3. Read the `Overflows` Create request and PUT a SUCCESS response padded to exactly 4097 bytes (the PUT is accepted, HTTP 200).
4. Poll the queue until a Delete request for `Overflows` arrives. Inspect everything else pending on the queue at that instant: a Delete request for `Fits` is already there. Expected: nothing for `Fits` yet — its Delete is published only after the `Overflows` Delete.
5. Answer both Deletes with SUCCESS and settle: the stack should reach ROLLBACK_COMPLETE with exactly one `Overflows` CREATE_FAILED event whose `ResourceStatusReason` is `Response object is too long.`
