Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__redshift.cfn-redshift-clusterparametergroup-invalid-family-rejected
Issue-Key: cloudformation__redshift.cfn-redshift-clusterparametergroup-invalid-family-rejected|run#9|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: 39c6dbb2798d7d5c6447151f9d119a36c56109b9

# cloudformation__redshift: an invalid AWS::Redshift::ClusterParameterGroup ParameterGroupFamily now rolls the stack back, but the resource failure reason carries the emulator's own wording and HandlerErrorCode InvalidRequest instead of AWS's passed-through Redshift exception with GeneralServiceException (1 cause)

> _Note: this issue was identified against the code base at commit `39c6dbb2798d7d5c6447151f9d119a36c56109b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the resource failure reason CloudFormation records when an AWS::Redshift::ClusterParameterGroup declares an unknown ParameterGroupFamily has the wrong message text and the wrong HandlerErrorCode.

AWS: Smithy (CreateClusterParameterGroup) models ParameterGroupFamily as a required String with no enumerated domain and declares NO error for an out-of-domain family (its modeled errors are ClusterParameterGroupAlreadyExistsFault, ClusterParameterGroupQuotaExceededFault, InvalidTagFault, TagLimitExceededFault) — so the refusal is recorded behavior only: Redshift answers HTTP 400 with the message 'ParameterGroupFamily <family> is not a valid parameter group family', where 'ParameterGroupFamily ' and ' is not a valid parameter group family' are the fixed text and <family> is the value taken verbatim from the request. CloudFormation passes that service exception through into the resource's ResourceStatusReason as 'Resource handler returned message: "<service message> (Service: Redshift, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: GeneralServiceException)' — the '(Service: Redshift, Status Code: 400, Request ID: …) (SDK Attempt Count: …)' decoration and HandlerErrorCode GeneralServiceException are fixed for a service exception the handler did not itself classify; the request id, attempt count and request token are per-run values the check normalizes.

Emulator: the stack now does settle ROLLBACK_COMPLETE (progress over the previously filed state), but the recorded handler reason is message "Invalid value for ParameterGroupFamily: '<family>'. Valid Values: redshift-1.0 | redshift-2.0" with HandlerErrorCode InvalidRequest: AWS's fixed text is replaced by the emulator's own validation wording plus an invented 'Valid Values: …' list, the SDK '(Service: Redshift, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)' decoration is absent, and the handler classifies the failure itself as InvalidRequest instead of surfacing it as a passed-through GeneralServiceException.

Why: a client that reads stack events to diagnose a rejected template gets text that does not match AWS's, so log scraping and error assertions on the documented message fail; and HandlerErrorCode InvalidRequest vs GeneralServiceException changes how CloudFormation (and tooling built on it) classifies the failure — a handler-side request defect versus a downstream service refusal — which drives retry/rollback decisions. The missing '(Service: Redshift, Status Code: 400, …)' decoration also hides which service and status actually refused the value.

Fix: the AWS::Redshift::ClusterParameterGroup resource handler's error mapping (pass the Redshift exception through unmodified, with the SDK decoration, and report HandlerErrorCode GeneralServiceException) and Redshift's CreateClusterParameterGroup ParameterGroupFamily validation message, which must use AWS's 'ParameterGroupFamily <family> is not a valid parameter group family' wording with the family echoed from the request and no 'Valid Values' suffix.

Note: open issue 20261109T125831Z-cloudformation-redshift-4-net-new-causes-aws-redshift.md (Cause 4) describes the earlier state of this check, where the stack reached CREATE_COMPLETE; it does not carry this failure's replay key (the check now fails on the handler-reason assertion, not on the stack status), so this message/code divergence is filed separately. Note also the identical mapping defect for AWS::Redshift::ClusterSubnetGroup recorded as Cause 2 of that issue — the same fix in the handler error mapping likely covers both.

Traces: cloudformation__redshift.cfn-redshift-clusterparametergroup-invalid-family-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 (cloudformation__redshift 0/6 passed).

[mismatch] cloudformation__redshift.cfn-redshift-clusterparametergroup-invalid-family-rejected:
  AssertionError: handler reason ("Invalid value for ParameterGroupFamily: 'redshift-9.9'. Valid Values: redshift-1.0 | redshift-2.0", 'InvalidRequest'), expected ('ParameterGroupFamily redshift-9.9 is not a valid parameter group family (Service: Redshift, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'GeneralServiceException')

The check parses the resource's ResourceStatusReason with the regex ^Resource handler returned message: "(?P<message>.*)" \(RequestToken: [0-9a-f-]+, HandlerErrorCode: (?P<code>[A-Za-z]+)\)$ and compares the (message, code) pair; <id> and <n> are placeholders the check normalizes, so the request id and attempt count are NOT part of the divergence. 'redshift-9.9' is this run's invalid family value (the positive control uses 'redshift-2.0'); the requirement is the message template with the requested family substituted.

Diverging parts: (a) message text — AWS fixed text 'ParameterGroupFamily ' + <family> + ' is not a valid parameter group family' vs the emulator's "Invalid value for ParameterGroupFamily: '<family>'. Valid Values: redshift-1.0 | redshift-2.0"; (b) the missing fixed SDK decoration '(Service: Redshift, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)'; (c) HandlerErrorCode InvalidRequest instead of GeneralServiceException.

## Steps to Reproduce
1. CreateStack with a template declaring a single AWS::Redshift::ClusterParameterGroup whose ParameterGroupFamily is an unknown family (e.g. 'redshift-9.9') and a valid Description.
2. Poll DescribeStacks by StackId until terminal: the stack settles ROLLBACK_COMPLETE.
3. Read the failing resource's ResourceStatusReason from DescribeStackEvents: it reads 'Resource handler returned message: "Invalid value for ParameterGroupFamily: '"'"'redshift-9.9'"'"'. Valid Values: redshift-1.0 | redshift-2.0" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' instead of 'Resource handler returned message: "ParameterGroupFamily redshift-9.9 is not a valid parameter group family (Service: Redshift, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: GeneralServiceException)'.
4. Re-replay cloudformation__redshift.cfn-redshift-clusterparametergroup-invalid-family-rejected.
