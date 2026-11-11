Issue-Source: trace-sweep-divergence
Issue-Trace: eks-ekscc-traj-create-request-shape-mistakes
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-ekscc-traj-create-request-shape-mistakes
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateCluster|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|CreateCluster|4xx->4xx|status
Issue-Related: 20261104T090203Z-eks-createcluster-refuses-with-generic-framework-wording-instead.md
Issue-Commit: 0e97705e9904239bd55bed92c4be71546a783c93

# EKS CreateCluster answers an unauthorized call with a raw deserializer ValidationException instead of 403

> _Note: this issue was identified against the code base at commit `0e97705e9904239bd55bed92c4be71546a783c93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
ONE net-new root cause: CreateCluster deserializes the request body before it evaluates the caller's authorization, and when the body cannot be bound it surfaces the JSON deserializer's internal diagnostic as a modeled fault. The authorization failure AWS reports is therefore never reached.

Cause 1 of 1: an unauthorized CreateCluster whose body also has a shape problem is answered HTTP 400 with x-amzn-errortype 'ValidationException' and the deserializer's own sentence, where AWS answers HTTP 403 with its not-authorized sentence.
  AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json) CreateCluster POST /clusters, success 200, output cluster -> Cluster; input name (@required, ClusterName, @length 1..100, @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$), roleArn (@required), resourcesVpcConfig (@required, VpcConfigRequest) and the optional members; modeled errors ClientException 400, InvalidParameterException 400, ResourceInUseException 409, ResourceLimitExceededException 400, UnsupportedAvailabilityZoneException 400, ServerException 500, ServiceUnavailableException 503 — there is NO ValidationException on this operation, so that error type is unmodeled on the wire. For this request the recorded response is HTTP 403 with the message 'User: <principalArn> is not authorized to perform: eks:CreateCluster' — fixed text 'User: ' and ' is not authorized to perform: eks:CreateCluster', with the caller's principal ARN as the only variable part, taken from the signing credentials (the recorded run's arn:aws:iam::000000000000:user/emulator is an environment value, not a requirement). Hypothesis (recorded behavior only; the model does not state ordering): AWS evaluates IAM authorization for eks:CreateCluster before binding the request body, so a body-shape problem cannot mask the access-control refusal.
  Emulator: HTTP 400, x-amzn-errortype 'ValidationException', $.message 'expected array start, got string' — a raw JSON-deserializer diagnostic that is not AWS wording, names no input member, and is returned under an error type CreateCluster does not model.
  Why: the client is told its request is malformed in deserializer terms when AWS would tell it the principal lacks eks:CreateCluster. A caller that classifies 403/AccessDeniedException as an IAM problem and 400 as a non-retryable request bug fixes the body forever and never fixes the policy; and anything matching on EKS's not-authorized sentence (alarms, retry/escalation logic, operator-facing output) sees an unrecognised string, so the authorization gap is invisible.
  Fix: CreateCluster's handler — evaluate caller authorization before the request body is deserialized, and the request deserializer must never surface a raw parser diagnostic as a modeled fault (no unmodeled ValidationException on this operation).
  Traces: eks-ekscc-traj-create-request-shape-mistakes#task#10.

Related OPEN issue that does NOT own this replay unit: '20261104T090203Z-eks-createcluster-refuses-with-generic-framework-wording-instead.md' describes the same family of CreateCluster refusal faults, but its replay key covers the 4xx->4xx $.message facets of CreateCluster, not the 403->400 status/error-type facet diverging here, and this replay reports the cluster as owned by no open issue. It is filed separately so it is replayed and closed on its own evidence; a fixer closing both should verify this step passes independently.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, 55 fixtures replayed, 393 steps (271 passed, 122 mismatched, 0 errored, 0 blocked), score 69.0%, 24 divergence clusters. This cluster was reported with empty owned_by and empty stub_path.

Divergence cluster key: service=eks|CreateCluster|status=403->400|keys=$.message,$headers.x-amzn-errortype,status — 1 affected step.
Representative step: CreateCluster, fixture eks-ekscc-traj-create-request-shape-mistakes, step task#10.
  Expected: status 403, $.message 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: eks:CreateCluster' (fixed text 'User: ' / ' is not authorized to perform: eks:CreateCluster'; variable part: the caller's principal ARN, from the request's signing credentials — the account id and user name in the recorded ARN are environment values).
  Actual:   status 400, $headers.x-amzn-errortype 'ValidationException', $.message 'expected array start, got string' (an internal deserializer diagnostic; whole string is emulator-authored and names no member).

Smithy model consulted: CreateCluster http POST /clusters, success code 200; input name -> ClusterName @required @length 1..100 @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$, roleArn @required, resourcesVpcConfig @required; errors ClientException 400, InvalidParameterException 400, ResourceInUseException 409, ResourceLimitExceededException 400, UnsupportedAvailabilityZoneException 400, ServerException 500, ServiceUnavailableException 503 — no ValidationException is modeled on this operation.

## Steps to Reproduce
1. git clone the project repository and build the emulator from this checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Inspect fixture eks-ekscc-traj-create-request-shape-mistakes, step task#10: the emulator answers 400 ValidationException 'expected array start, got string' where the recording has 403 'User: <principalArn> is not authorized to perform: eks:CreateCluster'. Re-replay that step to confirm a fix.
4. Manually, against the emulator's EKS endpoint: POST /clusters as a principal with no eks:CreateCluster permission, with a body in which a list-typed member (e.g. resourcesVpcConfig.subnetIds) is given a string instead of an array. The emulator answers 400 with the unmodeled ValidationException and the deserializer sentence; AWS answers 403 with the not-authorized sentence naming the caller's principal ARN, because authorization is evaluated before the body is bound.
5. Repeat with a well-formed body and the same unauthorized principal to confirm the 403 path and its exact wording.
