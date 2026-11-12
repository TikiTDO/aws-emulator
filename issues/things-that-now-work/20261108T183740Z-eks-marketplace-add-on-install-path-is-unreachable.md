Issue-Source: trace-sweep-divergence
Issue-Trace: eks-efsfips-traj-marketplace-fips-addon-detour
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-efsfips-traj-marketplace-fips-addon-detour
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateAddon|2xx->4xx|$.addon
Issue-Key: eks|CreateAddon|2xx->4xx|status
Issue-Key: eks|DescribeAddon|2xx->4xx|$.addon
Issue-Key: eks|DescribeAddon|2xx->4xx|status
Issue-Key: eks|DeleteAddon|2xx->4xx|$.addon
Issue-Key: eks|DeleteAddon|2xx->4xx|status
Issue-Commit: 99cd58a9732d3abbd4ae376bbbd8ca320e363114

# eks marketplace add-on install path is unreachable: CreateAddon rejects an unsubscribed Marketplace add-on instead of recording CREATE_FAILED

> _Note: this issue was identified against the code base at commit `99cd58a9732d3abbd4ae376bbbd8ca320e363114`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAddon

## Description
Three net-new root causes share one fault: the emulator has no state for an AWS Marketplace-published add-on, so CreateAddon refuses the install outright instead of accepting it and recording a failed add-on, and the two follow-on reads/mutations of that add-on then cannot find it.

Cause 1 of 3: CreateAddon refuses a Marketplace add-on the account is not subscribed to, instead of creating it in CREATE_FAILED.
  AWS: CreateAddon (Smithy: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json, POST /clusters/{clusterName}/addons, httpResponseCode 200, output CreateAddonResponse.addon -> Addon) ACCEPTS the request for a Marketplace add-on whose subscription is missing: it answers HTTP 200 with an Addon whose status is CREATE_FAILED (Smithy: AddonStatus enum includes CREATE_FAILED) and whose health.issues carries one issue with code 'AddonSubscriptionNeeded' (Smithy: AddonIssueCode enum includes AddonSubscriptionNeeded) and the message 'You are currently not subscribed to this add-on. To subscribe, visit the AWS Marketplace console, agree to the seller EULA, select the pricing type if required, then re-install the add-on' — entirely fixed text, with no variable part. The returned Addon also carries addonName and clusterName echoed from the request, an addonArn of the form arn:aws:eks:<region>:<account>:addon/<clusterName>/<addonName>/<uuid>, createdAt/modifiedAt timestamps, tags {} and namespaceConfig.namespace 'kube-system'. Subscription state is a precondition on the ADD-ON's health, not on the request's admissibility.
  Emulator: HTTP 400 with no addon member at all — the call is refused, so no add-on resource is ever created.
  Why: a client installing a Marketplace add-on is supposed to read the AddonSubscriptionNeeded health issue off the created add-on and then subscribe in Marketplace and re-install; with a bare 400 it has no add-on to inspect, no issue code to branch on, and the documented remediation path is unreachable. Any workflow that polls the add-on it just created breaks at the first step.
  Fix: CreateAddon's handler and state — admit Marketplace-owned add-on names, persist an Addon in CREATE_FAILED with the AddonSubscriptionNeeded health issue, rather than rejecting the request in validation.
  Traces: eks-efsfips-traj-marketplace-fips-addon-detour#task#7.

Cause 2 of 3: DescribeAddon cannot find the Marketplace add-on, so its health is never readable.
  AWS: DescribeAddon (GET /clusters/{clusterName}/addons/{addonName}, httpResponseCode 200) returns the add-on created above with status CREATE_FAILED and the same health.issues[0].code 'AddonSubscriptionNeeded' and message quoted in Cause 1; ResourceNotFoundException (404) is reserved for an add-on that does not exist.
  Emulator: HTTP 404 with no addon member — the add-on is absent from state because Cause 1 never created it.
  Why: the client's poll of the add-on it believes it created reports not-found, which it must interpret as 'someone deleted it' rather than 'subscription needed'; the failure is misattributed and the AddonSubscriptionNeeded signal never reaches the caller.
  Fix: EKS add-on state (the same store CreateAddon writes) and DescribeAddon's lookup.
  Traces: eks-efsfips-traj-marketplace-fips-addon-detour#task#8.

Cause 3 of 3: DeleteAddon cannot find the Marketplace add-on, so the failed install cannot be cleaned up.
  AWS: DeleteAddon (DELETE /clusters/{clusterName}/addons/{addonName}, httpResponseCode 200) answers 200 with the Addon transitioned to status DELETING (Smithy: AddonStatus enum includes DELETING), preserving addonName, clusterName, addonArn and the health.issues entry; a CREATE_FAILED add-on is deletable.
  Emulator: HTTP 404 with no addon member.
  Why: a client cannot roll back or retry the install — it has nothing to delete and nothing to re-create, so the add-on name stays in an indeterminate state for any orchestration built on create/delete idempotency.
  Fix: EKS add-on state and DeleteAddon's lookup/status transition.
  Traces: eks-efsfips-traj-marketplace-fips-addon-detour#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All three clusters report stub_path empty and owned_by empty (not covered by any open issue's replay key).

Cause 1 — eks-efsfips-traj-marketplace-fips-addon-detour#task#7, CreateAddon: status expected 200, got 400; $.addon missing — expected an Addon with addonName 'chainguard_aws-efs-csi-driver-fips', clusterName <the probe cluster>, status 'CREATE_FAILED', health.issues[0].code 'AddonSubscriptionNeeded', health.issues[0].message 'You are currently not subscribed to this add-on. To subscribe, visit the AWS Marketplace console, agree to the seller EULA, select the pricing type if required, then re-install the add-on', addonArn arn:aws:eks:<region>:<account>:addon/<cluster>/<addonName>/<uuid>, tags {}, namespaceConfig.namespace 'kube-system' (addonVersion/createdAt/modifiedAt shaped-not-asserted); got None.

Cause 2 — eks-efsfips-traj-marketplace-fips-addon-detour#task#8, DescribeAddon: status expected 200, got 404; $.addon missing — expected the same add-on with status 'CREATE_FAILED' and the AddonSubscriptionNeeded health issue.

Cause 3 — eks-efsfips-traj-marketplace-fips-addon-detour#task#9, DeleteAddon: status expected 200, got 404; $.addon missing — expected the same add-on with status 'DELETING' and the AddonSubscriptionNeeded health issue.

Smithy consulted: CreateAddon / DescribeAddon / DeleteAddon (http 200, output .addon -> Addon, errors ResourceNotFoundException 404, InvalidParameterException 400); Addon members status/health/addonArn/marketplaceInformation/namespaceConfig; AddonStatus enum (CREATING, ACTIVE, CREATE_FAILED, UPDATING, DELETING, ...); AddonIssueCode enum (..., AddonSubscriptionNeeded, ...).

## Steps to Reproduce
1. git clone the project repo; the emulator is built from that checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Representative divergence: fixture eks-efsfips-traj-marketplace-fips-addon-detour, step task#7; the two follow-on causes are task#8 (DescribeAddon) and task#9 (DeleteAddon) of the same fixture.
4. Manually: create a cluster, then POST /clusters/<cluster>/addons with addonName 'chainguard_aws-efs-csi-driver-fips' (a Marketplace-published add-on with no subscription) — expect 200 and an Addon in CREATE_FAILED carrying health issue code AddonSubscriptionNeeded; the emulator answers 400 with no addon. Then GET and DELETE /clusters/<cluster>/addons/chainguard_aws-efs-csi-driver-fips — expect 200 (status CREATE_FAILED, then DELETING); the emulator answers 404.
