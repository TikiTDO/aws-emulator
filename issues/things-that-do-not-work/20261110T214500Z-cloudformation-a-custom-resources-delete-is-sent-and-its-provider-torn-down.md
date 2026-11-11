Issue-Severity: HIGH

# cloudformation: a stack teardown sends a custom resource's `Delete` to its provider and then deletes the provider and the resources it depends on without waiting for the provider's answer, so the resource always fails to delete (1 cause)

> _Note: this issue was filed from a reproduction written while fixing
> `20261109T185037Z-cloudformation-lambda-s3-sqs-the-in-lambda-aws-sdk-binds-no-s3.md`; the
> code base you are working in can have drifted, so trace it back to the current one._

## Operation
DeleteStack

## Description
Cause 1 of 1: the delete walk treats "the provider has been SENT the `Delete`" as the end of
that resource's teardown and carries straight on to the resources declared before it, so a
Lambda-backed custom resource's provider function — and the resource the provider's handler
acts on — are deleted while the provider request is still in flight. The invocation then never
runs, nothing answers at the request's `ResponseURL`, the removal barrier runs out, and the
stack reports `DELETE_FAILED` / `ROLLBACK_FAILED` with "The following resource(s) failed to
delete: [<the custom resource>]".

AWS: CloudFormation deletes a stack's resources in dependency order and does not begin
deleting a resource while a resource that DEPENDS on it is still being deleted — a custom
resource's delete is complete only when its provider answers at the `ResponseURL`
(`DeleteStack`, and the custom-resource provider contract). The one CDK pattern this is most
visible in is `bucket.addEventNotification(...)`: the generated
`Custom::S3BucketNotifications` resource depends on its provider function and on the bucket,
and on a destroy or a rollback its `Delete` runs — replacing the bucket's notification
container — before either is removed.

Emulator: `crate::state::provisioning::delete_pass`
(`services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning.rs`)
walks the resources newest-first. For a custom resource it calls `custom_resource::send(...)`,
pushes a `Stabilising { awaited: Awaited::Gone, provider_request: Some(..) }` and then
CONTINUES the loop, so the provider function (`AWS::Lambda::Function`) and the resource the
provider's handler administers are deleted in the same pass. The asynchronous (`Event`)
invocation the provider was sent therefore never runs — the function is gone — and the barrier
fails at `REMOVAL_STABILISATION_WINDOW` with
`resource_failure_summary(failed_action::DELETE, ..)`.

Why: no stack that deploys a Lambda-backed custom resource can be deleted, and no stack that
deploys one can roll back: the rollback of an unrelated resource's failure leaves the stack in
`ROLLBACK_FAILED` on the custom resource, which is what a `cdk deploy` of the standard
bucket-notification app reports even when every resource of it was created.

Fix: a resource whose removal is still open must hold back the deletion of the resources it
NEEDS (`Teardown::needs` already carries that graph, and `needed_by` already defers the
resources a REFUSED delete needs) until its barrier is released — so the provider, and the
resource the provider acts on, are deleted only after the provider has answered at the
`ResponseURL`.

## Evidence
`core/router/tests/all/cloudformation_cdk_bucket_notifications_custom_resource.rs` deploys a
stack holding a queue, its queue policy, a bucket, a provider function in the shape CDK's
notifications handler has, and a `Custom::S3BucketNotifications` that `DependsOn` the policy
and the function. The stack reaches `CREATE_COMPLETE` and the notification is delivered.
Adding a `DeleteStack` to the end of that suite reports:

```
StackStatus DELETE_FAILED
StackStatusReason The following resource(s) failed to delete: [BucketNotifications].
```

with no `ResourceStatusReason` on the resource itself, and the provider function's log group
holding only the CREATE invocation — the `Delete` invocation never ran. That stage is left out
of the suite because it asserts this defect rather than the one that suite covers.

## Steps to Reproduce
1. Deploy a stack with a Lambda-backed `Custom::<Name>` resource that `DependsOn` its provider
   function, where the provider's handler calls the service that owns another resource of the
   same stack.
2. `DeleteStack` (or let the stack roll back).
3. Expect `DELETE_COMPLETE`; the stack reports `DELETE_FAILED` naming the custom resource, and
   the provider was never invoked for the `Delete`.
