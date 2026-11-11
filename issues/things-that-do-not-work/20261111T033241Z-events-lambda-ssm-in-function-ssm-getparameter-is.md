Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__ssm.lambda-without-ssm-permission-denied events__lambda__ssm.lambda-ssm-wrong-resource-arn-denied events__lambda__ssm.lambda-ssm-path-prefix-condition-restriction-denied
Issue-Key: events__lambda__ssm.lambda-without-ssm-permission-denied|run#14|AssertionError|assert payload["denied"] is True, f"expected AccessDenied, got {payload}"
Issue-Key: events__lambda__ssm.lambda-ssm-wrong-resource-arn-denied|run#13|AssertionError|assert payload["other"] == "AccessDeniedException", (
Issue-Key: events__lambda__ssm.lambda-ssm-path-prefix-condition-restriction-denied|run#14|AssertionError|assert payload["outside"] == "AccessDeniedException", (
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# events__lambda__ssm: in-function ssm:GetParameter is never authorized against the Lambda execution role — a function with no, scoped or path-conditioned SSM grant reads every parameter

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
One net-new root cause under the events__lambda__ssm heading, grounded in three failing checks that exercise three shapes of the same gap.

Cause 1 of 1: an ssm:GetParameter call made by a Lambda handler with the function's execution-role credentials is never authorized — it succeeds and returns the parameter value whether the role has no SSM statement at all, a statement scoped to a different parameter ARN, or a statement whose Condition restricts it to another path prefix.

AWS: Smithy (ssm) GetParameter models only InternalServerError, InvalidKeyId, ParameterNotFound and ParameterVersionNotFound — IAM authorization is the framework's refusal and is not an operation-modelled error, so its wire form is recorded behavior: the call is refused with error code 'AccessDeniedException' and the message 'User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: ssm:GetParameter on resource: <parameter ARN> because no identity-based policy allows the ssm:GetParameter action'. Everything in that sentence is fixed text except the four values taken from the caller and the request: the account id, the execution role's name, the assumed-role session name (for a Lambda invocation this is the function name), and the parameter's ARN (arn:aws:ssm:<region>:<account>:parameter/<name without its leading slash>). No Parameter is returned, so the value never reaches the handler. The same refusal is the required outcome when the role's Resource names another parameter ARN, and when its Condition (an ssm path-prefix restriction) excludes the requested name; a parameter the policy DOES cover must still be readable in the same invocation, which is what proves the denial is permission-specific rather than a blanket failure.

Emulator: every call is allowed. The no-permission function returns {'ok': True, 'denied': False, 'value': '<parameter value>'} — the secret value is handed to a handler whose role grants nothing. The resource-scoped function returns 'allowed' for both the in-scope and the out-of-scope parameter, and the path-prefix-conditioned function returns 'allowed' for both the inside and the outside parameter. No AccessDeniedException is ever produced, so neither the code nor the message exists.

Why: this is a security-relevant false negative. A client validating least-privilege (a deploy-time test that asserts a function CANNOT read a parameter it has no grant for, a guardrail test for a path-scoped policy, a blast-radius review) passes against the emulator and fails in AWS — or, worse, ships a function that in AWS is denied at runtime after the emulator proved it 'works'. Any handler branch written for AccessDeniedException is dead code here.

Fix: the Lambda execution-role credential path and the SSM request authorizer behind it — GetParameter (and its siblings) must evaluate the caller's identity policy, including Resource ARNs and Conditions, and refuse with AccessDeniedException and that message instead of serving the parameter.

Related open issue in the same family that does NOT carry these replay keys: 20261106T151148Z-events-lambda-ssm-in-function-events-putevents-is.md (in-function events:PutEvents accepted with no permission).

Traces: events__lambda__ssm.lambda-without-ssm-permission-denied; events__lambda__ssm.lambda-ssm-wrong-resource-arn-denied; events__lambda__ssm.lambda-ssm-path-prefix-condition-restriction-denied.

## Evidence
events__lambda__ssm.lambda-without-ssm-permission-denied (line 95):
  assert payload["denied"] is True
  AssertionError: expected AccessDenied, got {'ok': True, 'denied': False, 'value': 'secret-value'}
  The check's next assertions require payload['code'] == 'AccessDeniedException' and payload['message'] == "User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: ssm:GetParameter on resource: arn:aws:ssm:<region>:<account>:parameter/<name> because no identity-based policy allows the ssm:GetParameter action", and that no 'value' is present.

events__lambda__ssm.lambda-ssm-wrong-resource-arn-denied (line 104):
  AssertionError: expected AccessDeniedException for out-of-scope parameter, got {'allowed': 'allowed', 'other': 'allowed'}

events__lambda__ssm.lambda-ssm-path-prefix-condition-restriction-denied (line 105):
  AssertionError: out-of-scope parameter should be denied with AccessDeniedException, got {'inside': 'allowed', 'outside': 'allowed'}

Expected vs actual: expected error code 'AccessDeniedException' with the fixed sentence above (variable parts: account id, role name, session name = function name, parameter ARN) and no Parameter in the response; actual HTTP 200 with the Parameter and its Value.

## Steps to Reproduce
1. PutParameter a String parameter with a known value (repeat with a hierarchical name such as /sim2real/<run>/inside and a second one outside that path).
2. CreateRole trusting lambda.amazonaws.com. Variant A: attach NO ssm statement. Variant B: attach ssm:GetParameter scoped by Resource to parameter A's ARN only. Variant C: attach ssm:GetParameter with a Condition restricting it to one path prefix.
3. CreateFunction (python runtime) whose handler calls ssm.get_parameter(Name=<parameter>) inside try/except ClientError and returns the error Code and Message, or the value on success.
4. Invoke the function for each parameter.
   Expected: the parameter the policy does not cover is refused with code 'AccessDeniedException' and the message above, and no value is returned; the covered parameter still reads successfully in variants B and C.
   Emulator: every invocation returns the parameter value ('allowed' / 'ok': True).
