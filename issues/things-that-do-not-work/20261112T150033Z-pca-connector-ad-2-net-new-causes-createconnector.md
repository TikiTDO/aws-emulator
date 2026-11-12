Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pca-connector-ad.create-connector-invalid-certificate-authority-arn-pattern-rejected pca-connector-ad.create-connector-missing-iam-permission-rejected
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad.create-connector-invalid-certificate-authority-arn-pattern-rejected|run#13|AssertionError|assert message == (
Issue-Key: pca-connector-ad.create-connector-missing-iam-permission-rejected|run#20|Timeout|time.sleep(3)
Issue-Commit: b674738a58d9d62ba939f6eca5e8d4760a2144d8

# pca-connector-ad: 2 net-new causes — CreateConnector's CertificateAuthorityArn @pattern rejection uses the generic "1 validation error detected" template, and CreateConnector performs no IAM authorization (an explicit Deny never yields AccessDeniedException)

> _Note: this issue was identified against the code base at commit `b674738a58d9d62ba939f6eca5e8d4760a2144d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnector

## Description
Two net-new root causes in the pca-connector-ad emulator's CreateConnector path, each grounded in a failing behavioral check of this replay that the suite did NOT mark as already filed or OWNED. Service model: services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/smithy-model/com-amazonaws-pcaconnectorad-pcaconnectorad.json.

Related open issue, named because it looks similar but does not own either check below: 20261109T225602Z-pca-connector-ad-createconnector-modeled-constraint-rejections-use-the-generic-1.md. Its Cause 1 describes the same message-template defect for CreateConnector's DirectoryId/SecurityGroupIds members; this issue's Cause 1 is the CertificateAuthorityArn member, whose check the suite left unowned (that issue's replay key for this check was the Cause-2 'call succeeded' failure, which no longer occurs — the unregistered-directory control now correctly answers 404 ResourceNotFoundException with the exact message, and the check now fails one assertion later, on the @pattern message). Its Cause 2 is unrelated to anything here. No open issue mentions pca-connector-ad authorization at all, so Cause 2 below is wholly net-new.

=== Cause 1 of 2: CreateConnector renders a CertificateAuthorityArn @pattern violation with the cross-service "1 validation error detected" template instead of this service's bracketed ECMA-262 wording ===
AWS: CreateConnector refuses a CertificateAuthorityArn that fails its modeled regular expression with HTTP 400 and error code ValidationException (Smithy: CreateConnector declares ValidationException @httpError 400; CreateConnector.CertificateAuthorityArn -> CertificateAuthorityArn @required, @length min 5 max 200, @pattern "^arn:[\\w-]+:acm-pca:[\\w-]+:[0-9]+:certificate-authority\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$"). The message is the bracketed JSON-schema-validator form, with NO member path and NO "1 validation error detected" prefix (message text: recorded behavior only): `[ECMA 262 regex "<the pattern exactly as the model declares it, anchors ^ and $ included>" does not match input string "<the rejected value from the request>"]`. Fixed text: `[ECMA 262 regex `, ` does not match input string `, `]`, plus the double-quoting of both variable parts; variables: the modeled pattern for the member that failed, and the value the request supplied for it.
Emulator: the HTTP status (400) and the error code (ValidationException) are correct; only the message diverges. For CertificateAuthorityArn="not-an-acm-pca-arn" it returns `1 validation error detected: Value 'not-an-acm-pca-arn' at 'certificateAuthorityArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\\w-]+:acm-pca:[\\w-]+:[0-9]+:certificate-authority\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}` — the generic cross-service template, additionally stripping the pattern's ^ and $ anchors that AWS echoes verbatim.
Why: a client that classifies on or surfaces the rejection text (error-handling middleware, operator diagnostics, regression assertions) sees a message real AWS never sends for this operation, and loses the bracketed regex/offending-input detail AWS puts there. The defect is per-member, so fixing it only for DirectoryId/SecurityGroupIds leaves CertificateAuthorityArn (and every other @pattern/@length member of this service) still wrong.
Fix: the per-service ValidationException message renderer in pca-connector-ad's input-validation layer — it must format EVERY modeled @pattern/@length violation of this service in the bracketed form, driven by the member's modeled constraint, instead of delegating to the generic cross-service template.
Traces: pca-connector-ad.create-connector-invalid-certificate-authority-arn-pattern-rejected.

=== Cause 2 of 2: CreateConnector applies no identity-based IAM authorization — a principal carrying an explicit Deny on pca-connector-ad:CreateConnector is never refused with AccessDeniedException ===
AWS: CreateConnector is authorized against the caller's identity-based policies before any request validation or resource lookup. A session whose inline policy contains an explicit Deny on pca-connector-ad:CreateConnector is refused with HTTP 403 and error code AccessDeniedException (Smithy: CreateConnector declares AccessDeniedException @httpError 403), and nothing is created. The message is AWS's standard IAM denial sentence: `User: <the caller's assumed-role session ARN> is not authorized to perform: <service>:<Operation> on resource: <the resource ARN the action is evaluated against> with an explicit deny in an identity-based policy`. Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` with an explicit deny in an identity-based policy`; variables taken from the request's signing identity and the evaluated action/resource — the assumed-role session ARN (`arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>`), the action `pca-connector-ad:CreateConnector`, and the connector resource ARN `arn:aws:pca-connector-ad:<region>:<account>:connector/*`. The denial-sentence wording is AWS-documented IAM behavior; the specific resource ARN form for this action is recorded behavior only.
Emulator: no AccessDeniedException is ever produced for this call. The check assumes a role whose inline policy is `Allow pca-connector-ad:* + Deny pca-connector-ad:CreateConnector`, then re-issues CreateConnector under those session credentials in a bounded retry loop (to absorb IAM propagation) until the denial message appears. The loop never sees it and runs its full 60s deadline, so the check is killed by the harness: `Check timed out after 60.0s`. The emulator serves the request as if the principal were unrestricted, reaching the request-validation/resource-lookup path instead of the authorization gate.
Why: an overly-permissive authorization surface. A client or test that relies on IAM to contain CreateConnector gets no refusal at all: negative tests asserting the modeled 403 pass against the emulator and fail against AWS, least-privilege policies cannot be validated, and any denial-driven error handling (retry/alarm/escalate on AccessDeniedException) is never exercised. Because the emulator does not answer with the expected denial, the call also proceeds far enough to touch state AWS never lets it reach.
Fix: an identity-based authorization gate in the pca-connector-ad request pipeline, evaluated before CreateConnector's input validation and directory lookup: resolve the signed principal, evaluate its identity policies for `pca-connector-ad:CreateConnector` against the connector resource ARN, and on a matched explicit Deny return the modeled AccessDeniedException (403) with AWS's denial sentence naming the assumed-role session ARN, the action and that resource ARN.
Traces: pca-connector-ad.create-connector-missing-iam-permission-rejected.

## Evidence
Suite: 4/15 passed; pca-connector-ad 4/15 (27%), 11 failed — 9 skipped as OWNED by already-open issues, the 2 below clustered here.

Cause 1 (status 400 and code ValidationException correct; message wrong):
* pca-connector-ad.create-connector-invalid-certificate-authority-arn-pattern-rejected — CreateConnector(DirectoryId="d-1234567890", CertificateAuthorityArn="not-an-acm-pca-arn", VpcInformation={"SecurityGroupIds": ["sg-0123456789abcdef0"]})
  expected message: `[ECMA 262 regex "^arn:[\\w-]+:acm-pca:[\\w-]+:[0-9]+:certificate-authority\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$" does not match input string "not-an-acm-pca-arn"]`
  actual message:   `1 validation error detected: Value 'not-an-acm-pca-arn' at 'certificateAuthorityArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\\w-]+:acm-pca:[\\w-]+:[0-9]+:certificate-authority\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}`
  Harness: AssertionError raised at the `assert message == (...)` on the malformed-ARN arm. The check's positive-control arm (pattern-valid CA ARN, unregistered DirectoryId) PASSED in this run — it got ResourceNotFoundException with message `DirectoryRegistration was not found: d-1234567890.` — so the only remaining divergence in this check is the rejection text.

Cause 2 (authorization not enforced):
* pca-connector-ad.create-connector-missing-iam-permission-rejected — IAM role created with inline policy [{Allow pca-connector-ad:* on *}, {Deny pca-connector-ad:CreateConnector on *}], assumed via STS, then CreateConnector(DirectoryId="d-1234567890", CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={"SecurityGroupIds":["sg-0123456789abcdef0"]}, Tags={"sim2real":"pca-connector-ad"}) under the session credentials.
  expected: HTTP 403, AccessDeniedException, message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: pca-connector-ad:CreateConnector on resource: arn:aws:pca-connector-ad:<region>:<account>:connector/* with an explicit deny in an identity-based policy`, and no connector created (the account id, role name and session name are per-run values that fill the template; they are not part of the requirement)
  actual:   the expected denial never appears, so the check's bounded 60s retry-until-denied loop never breaks on it and the harness reports `Check timed out after 60.0s`. The denial sentence is the loop's only exit condition, so the timeout is the emulator's failure to deny, not a runner fault.

## Steps to Reproduce
Boot the emulator and point a botocore client at it (region us-west-2).

Cause 1:
1. Call CreateConnector(DirectoryId="d-1234567890", CertificateAuthorityArn="not-an-acm-pca-arn", VpcInformation={"SecurityGroupIds": ["sg-0123456789abcdef0"]}).
2. Observe 400/ValidationException (correct) but with message `1 validation error detected: Value 'not-an-acm-pca-arn' at 'certificateAuthorityArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ...` instead of `[ECMA 262 regex "^arn:[\\w-]+:acm-pca:[\\w-]+:[0-9]+:certificate-authority\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$" does not match input string "not-an-acm-pca-arn"]`.

Cause 2:
3. iam.CreateRole with a trust policy allowing sts:AssumeRole from arn:aws:iam::<account>:root, then iam.PutRolePolicy with [{"Effect":"Allow","Action":"pca-connector-ad:*","Resource":"*"},{"Effect":"Deny","Action":"pca-connector-ad:CreateConnector","Resource":"*"}].
4. sts.AssumeRole into that role and build a pca-connector-ad client from the returned session credentials.
5. Call CreateConnector(DirectoryId="d-1234567890", CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={"SecurityGroupIds":["sg-0123456789abcdef0"]}, Tags={"sim2real":"pca-connector-ad"}) with those credentials. The emulator never answers 403 AccessDeniedException with the explicit-deny sentence; AWS does, and creates nothing. ListConnectors must show no connector for that CA ARN either way.
6. Re-replay the two checks named in trace_ids to confirm the fix.
