Issue-Source: trace-sweep-divergence
Issue-Trace: iot-register-thing-with-certificate
Issue-Step: teardown#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-register-thing-with-certificate iot-vpce-traj-rollback-order-refusals
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DeleteCertificate|4xx->2xx|$
Issue-Key: iot|DeleteCertificate|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteCertificate|4xx->2xx|status
Issue-Key: iot|DeleteCertificate|2xx->4xx|$
Issue-Key: iot|DeleteCertificate|2xx->4xx|status
Issue-Key: iot|DeleteCertificate|4xx->4xx|$.message
Issue-Commit: b78c4779749c8e01cf02c8115fda52ca12207fbe

# iot DeleteCertificate attachment preconditions: 3 causes - attached certificate is deleted anyway, conflict precedence reversed, legitimate delete then 404s

> _Note: this issue was identified against the code base at commit `b78c4779749c8e01cf02c8115fda52ca12207fbe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCertificate

## Description
Three net-new divergences, all in iot DeleteCertificate's attached-resource precondition: a certificate that still has a thing attached is deleted instead of refused, the refusal that IS raised names the wrong attachment class, and the later legitimate delete of that certificate then fails as not-found. They share one root fault - the handler's pre-delete attachment check (which attachments it consults, in which order, and whether a failed check actually blocks the removal).

Cause 1 of 4 [1 of 3]: DeleteCertificate deletes a certificate that still has an IoT thing attached.
AWS: Smithy DeleteCertificate - DELETE /certificates/{certificateId}, success code 200 with an empty body; modeled errors include DeleteConflictException (httpError 409), documented as "You can't delete the resource because it is attached to one or more resources". The operation's own documentation states "A certificate cannot be deleted if it has a policy or IoT thing attached to it or if its status is set to ACTIVE". So while a principal attachment created by AttachThingPrincipal is still in place, the delete is refused. Recorded response: 409 with body message "Things must be detached before deletion (arn: arn:aws:iot:<region>:<account>:cert/<certificateId>)" - fixed text "Things must be detached before deletion (arn: " ... ")", the only variable part being the certificate's ARN built from the request's certificateId and the emulator's region/account (the recorded 64-hex id is one instance of that variable, not part of the template). The recorded x-amzn-errortype for this step is shape-asserted only, so the header's exact value is not part of this cause - its presence is.
Emulator: answers 200 with an empty body and no x-amzn-errortype header, and removes the certificate (detail: "status: expected 409, got 200; $: empty_body (expected {'message': 'Things must be detached before deletion (arn: ...)'}, got {}); $headers.x-amzn-errortype: missing").
Why: the guard that protects a credential still bound to a device is absent. A client that relies on DeleteConflictException to discover it must DetachThingPrincipal first never sees the conflict, so it skips the detach and leaves the thing referencing a certificate that no longer exists; the registry is left inconsistent and there is no 409 to key the DeleteConflictException path on at all.
Fix: the DeleteCertificate handler's pre-delete precondition check and the certificate state it mutates - consult thing-principal attachments before removing the certificate and, when any exist, raise DeleteConflictException (409, iot error-type header, AWS's "Things must be detached before deletion (arn: <certArn>)" message) and leave the certificate in place.
Traces: iot-register-thing-with-certificate#teardown#6 (cluster also covers iot-vpce-traj-rollback-order-refusals#task#11).

Cause 2 of 4 [2 of 3]: the subsequent, legitimate DeleteCertificate answers 404 ResourceNotFoundException instead of 200.
AWS: same Smithy binding - DELETE /certificates/{certificateId}, success code 200, empty body. After the thing principal and policies have been detached and the certificate set INACTIVE, the delete succeeds. Recorded response: 200 with an empty body.
Emulator: 404 with body {"__type": "ResourceNotFoundException", "message": "CertificateId <certificateId> does not exist"} (detail: "status: expected 200, got 404; $: value_mismatch (expected {}, got {'__type': 'ResourceNotFoundException', 'message': 'CertificateId ... does not exist'})").
Hypothesis: the certificate was already removed by the earlier, wrongly-permitted delete described in cause 1, so by the time the properly-sequenced delete arrives the id is gone. Either way the wire result is a diverging status and body on a delete AWS accepts.
Why: a teardown or cleanup sequence that follows AWS's documented order (detach, deactivate, delete) terminates in an error. Clients that treat ResourceNotFoundException as a hard failure abort their cleanup, and idempotent-cleanup code that distinguishes "never existed" from "deleted" records the wrong outcome.
Fix: the DeleteCertificate handler and the certificate store - with cause 1 fixed the certificate is still present at this point and the delete must answer 200 with an empty body; the handler must not report not-found for a certificate the account still holds.
Traces: iot-register-thing-with-certificate#teardown#9 (cluster also covers iot-vpce-traj-rollback-order-refusals#task#15).

Cause 3 of 4 [3 of 3]: when both a policy and a thing are attached, the 409 message names the policy attachment instead of the thing attachment.
AWS: Smithy DeleteCertificate error DeleteConflictException (httpError 409). The message identifies which attachment class blocks the delete, and when a thing is attached AWS reports the thing attachment: recorded response 409 with message "Things must be detached before deletion (arn: arn:aws:iot:<region>:<account>:cert/<certificateId>)" - fixed text "Things must be detached before deletion (arn: " ... ")" with the certificate ARN as the only variable part.
Emulator: 409 (status correct) with message "Certificate policies must be detached before deletion (arn: <certArn>)" (detail: "$.message: value_mismatch"). The precondition is detected but attributed to the wrong attachment class, i.e. the policy check is consulted ahead of the thing check.
Why: the message is a requirement of the error class and is the only thing telling the caller what to detach. A client that parses it, or a human following it, detaches policies, retries, and gets the same refusal - the blocking thing principal is never named, so the delete can never be made to succeed from the error alone.
Fix: the DeleteCertificate handler's precondition check ordering and its DeleteConflictException message construction - evaluate thing-principal attachments first (or report every blocking class with AWS's wording for a thing attachment when one exists) so the 409 message is "Things must be detached before deletion (arn: <certArn>)".
Traces: iot-vpce-traj-rollback-order-refusals#task#8.

Note: the open issue 20261108T051259Z-iot-certificate-plane-4-causes-deleted-certificates-stay.md covers other certificate-plane divergences (DescribeCertificate after delete, transferData projection, UpdateCertificate status, TransferCertificate validation wording) but carries none of these three steps' replay keys, so these causes are filed here.

## Evidence
Cause 1 - fixture iot-register-thing-with-certificate step teardown#6, DeleteCertificate while a thing principal is still attached: expected 409 with $.message "Things must be detached before deletion (arn: arn:aws:iot:<region>:<account>:cert/<certificateId>)" and an x-amzn-errortype header present; actual 200, empty body, no error-type header, certificate removed. Cluster also covers iot-vpce-traj-rollback-order-refusals#task#11.
Cause 2 - fixture iot-register-thing-with-certificate step teardown#9, DeleteCertificate after the detach/deactivate sequence: expected 200 with an empty body; actual 404 with {"__type": "ResourceNotFoundException", "message": "CertificateId <certificateId> does not exist"}. Cluster also covers iot-vpce-traj-rollback-order-refusals#task#15.
Cause 3 - fixture iot-vpce-traj-rollback-order-refusals step task#8, DeleteCertificate with both a policy and a thing attached: both sides 409, but expected $.message "Things must be detached before deletion (arn: <certArn>)" while the emulator sent "Certificate policies must be detached before deletion (arn: <certArn>)".
Model citations: DeleteCertificate DELETE /certificates/{certificateId} code 200, input certificateId @httpLabel required @length 64..64 @pattern ^(0x)?[a-fA-F0-9]+$, forceDelete @httpQuery default false; errors include DeleteConflictException (409), CertificateStateException (406), ResourceNotFoundException (404); DeleteConflictException doc "You can't delete the resource because it is attached to one or more resources"; DeleteCertificate doc "A certificate cannot be deleted if it has a policy or IoT thing attached to it or if its status is set to ACTIVE".

## Steps to Reproduce
1. Replay fixture iot-register-thing-with-certificate to step teardown#6: with the certificate still attached to a thing via AttachThingPrincipal, call DeleteCertificate - the emulator answers 200 and removes the certificate instead of 409 DeleteConflictException "Things must be detached before deletion (arn: <certArn>)".
2. Continue the same fixture to teardown#9: after detaching the principal and policies and setting the certificate INACTIVE, call DeleteCertificate - the emulator answers 404 ResourceNotFoundException "CertificateId <id> does not exist" instead of 200 with an empty body.
3. Replay fixture iot-vpce-traj-rollback-order-refusals to step task#8: with both a policy and a thing attached to the certificate, call DeleteCertificate and compare $.message - the emulator reports the policy attachment where AWS reports "Things must be detached before deletion (arn: <certArn>)".
