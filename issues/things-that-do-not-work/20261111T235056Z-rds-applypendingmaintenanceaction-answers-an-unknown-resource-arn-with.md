Issue-Source: trace-sweep-divergence
Issue-Trace: rds-maintenance-and-account-attributes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-maintenance-and-account-attributes
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ApplyPendingMaintenanceAction|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ApplyPendingMaintenanceAction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ApplyPendingMaintenanceAction|4xx->4xx|status
Issue-Commit: 390a7a320ceffb672f1a3503b89f057095aef319

# rds ApplyPendingMaintenanceAction answers an unknown resource ARN with 404 ResourceNotFoundFault instead of 400 InvalidParameterValue

> _Note: this issue was identified against the code base at commit `390a7a320ceffb672f1a3503b89f057095aef319`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyPendingMaintenanceAction

## Description
1 net-new root cause in the RDS control plane: ApplyPendingMaintenanceAction mis-classifies a well-formed ARN that names a resource which does not exist.

Cause 1 of 1: ApplyPendingMaintenanceAction called with a syntactically valid DB-instance ARN for an identifier that does not exist is refused with the wrong status, the wrong error code and the wrong message.

AWS: recorded behavior only. The Smithy model (com.amazonaws.rds#AmazonRDSv19, ApplyPendingMaintenanceAction) binds exactly three errors to this operation — InvalidDBClusterStateFault (httpError 400, awsQueryError code InvalidDBClusterStateFault), InvalidDBInstanceStateFault (httpError 400, awsQueryError code InvalidDBInstanceState) and ResourceNotFoundFault (httpError 404, awsQueryError code ResourceNotFoundFault) — and says nothing about the unknown-ARN case, so the model alone does not decide it. The recorded response does: for a well-formed ARN of the form arn:aws:rds:<region>:<account>:db:<identifier> whose identifier names no DB instance, RDS answers HTTP 400 with the Query-protocol ErrorResponse Error.Code `InvalidParameterValue` and Error.Message `Unknown resource: <resource-arn>`. In that message `Unknown resource: ` is the fixed text and the part after it is a value taken straight from the request's ResourceIdentifier member — the FULL ARN as the caller sent it, not the bare identifier. (Hypothesis: RDS validates ResourceIdentifier as a parameter value rather than resolving it as a resource, which is why an unresolvable-but-well-formed ARN is reported as a bad parameter value and not as a missing resource; the recording is the requirement either way.)

Emulator: the same call answers HTTP 404 with Error.Code `ResourceNotFoundFault` and Error.Message `Resource not found: nonexistent-db-probe` — a different status class, a different error code, and a message whose fixed text (`Resource not found: `) differs from AWS's and whose variable part is the bare identifier parsed out of the ARN rather than the ARN the request carried.

Why: a client relying on AWS's behavior gets its error handling inverted twice over. Code that branches on the HTTP status sees 404 where AWS sends 400, so middleware and SDK retry/error-classification layers route the failure down a different path. Code that branches on Error.Code — idempotent teardown and maintenance sweepers that swallow a specific code, or validation surfaces that map InvalidParameterValue to "you sent a bad ARN" — never matches, so a call that AWS reports as a rejected parameter is reported here as a resource that vanished (or vice versa), and the caller either retries forever or reports the wrong fault to its user. Message-matching clients fail as well, because both the fixed sentence and the interpolated value differ.

Note: open behavioral issue 20261111T084329Z-rds-four-control-plane-validation-and-representation-divergences (Cause 1 of 4) asserts the OPPOSITE direction for this same operation — that 404 ResourceNotFoundFault with `Resource not found: <identifier>` is correct and 400 InvalidParameterValue is the defect. The recorded wire fixture replayed here shows the emulator now serving exactly what that issue asked for, and diverging from the recording. That issue does not cover this divergence and should not be closed by reverting half of it: the recorded AWS response is the authority, so the not-found path for ApplyPendingMaintenanceAction must produce 400 / InvalidParameterValue / `Unknown resource: <arn>`.

Fix: the ARN-resolution / not-found path of the ApplyPendingMaintenanceAction handler in the rds service crate, together with the error construction it calls — the unresolvable-ResourceIdentifier branch must raise the 400 InvalidParameterValue parameter error carrying the request's ResourceIdentifier ARN verbatim, instead of the 404 ResourceNotFoundFault it raises today. Genuine state faults (InvalidDBInstanceState / InvalidDBClusterStateFault) and a real ResourceNotFoundFault on other operations are unaffected.

Traces: rds-maintenance-and-account-attributes#task#3.

## Evidence
Cause 1 — fixture rds-maintenance-and-account-attributes, step task#3, operation ApplyPendingMaintenanceAction (service rds, Query protocol):

  Expected (recorded AWS response): HTTP 400
    $.ErrorResponse.Error.Code    = 'InvalidParameterValue'
    $.ErrorResponse.Error.Message = 'Unknown resource: arn:aws:rds:us-east-1:000000000000:db:nonexistent-db-probe'

  Actual (emulator): HTTP 404
    $.ErrorResponse.Error.Code    = 'ResourceNotFoundFault'
    $.ErrorResponse.Error.Message = 'Resource not found: nonexistent-db-probe'

  Replay verdict: mismatch; divergence_key service=rds|ApplyPendingMaintenanceAction|status=400->404|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status

  Message decomposition: fixed text `Unknown resource: `; variable part = the request's ResourceIdentifier ARN (the arn:aws:rds:... string above is this recording's value filling that slot, not part of the requirement). The emulator's `Resource not found: ` prefix is not AWS's fixed text for this error class, and it substitutes the bare DB identifier for the ARN.

  Model citation: Smithy ApplyPendingMaintenanceAction errors = InvalidDBClusterStateFault (400), InvalidDBInstanceStateFault (400, awsQueryError InvalidDBInstanceState), ResourceNotFoundFault (404) — InvalidParameterValue is not bound to the operation in the model, hence "recorded behavior only" for the unknown-ARN classification.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus: replay_wire_corpus().
2. The divergence is fixture `rds-maintenance-and-account-attributes`, step `task#3`.
3. Manually: against the rds endpoint, call ApplyPendingMaintenanceAction with a well-formed but unresolvable DB ARN, e.g. ResourceIdentifier='arn:aws:rds:<region>:<account>:db:nonexistent-db-probe', ApplyAction='system-update', OptInType='immediate', where no DB instance with that identifier exists.
4. Read the HTTP status and the Query ErrorResponse body. Expected: 400 with Error.Code 'InvalidParameterValue' and Error.Message 'Unknown resource: <the ResourceIdentifier ARN exactly as sent>'. Observed: 404 with Error.Code 'ResourceNotFoundFault' and Error.Message 'Resource not found: <bare identifier>'.
5. Re-replay fixture rds-maintenance-and-account-attributes step task#3 to confirm a fix.
