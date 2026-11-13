Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-create-put-update-secret-without-client-request-token
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-create-put-update-secret-without-client-request-token
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.VersionIdsToStages.sim2real-crt-b47f28475ba1da15589ab211f097c781[*]
Issue-Commit: 11fd660bd2fa41a4434704eaab7edd342e3cc9b2

# VersionIdsToStages labels a server-minted token-less version AWSCURRENT so DescribeSecret demotes the caller's own version

> _Note: this issue was identified against the code base at commit `11fd660bd2fa41a4434704eaab7edd342e3cc9b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
One cause.

Cause 1 of 1: DescribeSecret reports the secret version the caller staked with its OWN ClientRequestToken as AWSPREVIOUS instead of AWSCURRENT, because the emulator admitted two version-creating writes that AWS refuses and moved AWSCURRENT onto a version id it minted itself (fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#6).

AWS: Smithy (services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json): DescribeSecret input SecretId -> SecretIdType, required, @length 1..2048; output member VersionIdsToStages -> SecretVersionsToStagesMapType, a map from SecretVersionIdType (@length 32..64) to SecretVersionStagesType (list @length 1..20 of SecretVersionStageType). The map reports, for every version that EXISTS, the staging labels currently attached to it. PutSecretValue.ClientRequestToken and UpdateSecret.ClientRequestToken are both ClientRequestTokenType, @idempotencyToken, @length 32..64, and both operations model InvalidRequestException (HTTP 400); AWS refuses a raw HTTP request that omits the token on those operations, so in this scenario NO new version is ever created and the only version of the secret is the one whose id is the caller's own token. Recorded response for this step: HTTP 200 with that version id mapped to ["AWSCURRENT"]. The requirement is the LABEL and the arity: after two refused writes exactly one version exists and it carries AWSCURRENT. The map key is whatever ClientRequestToken value the caller sent — in this run sim2real-crt-b47f28475ba1da15589ab211f097c781, a per-run value that is not part of the requirement. DescribeSecret's modeled errors are InternalServiceError (500), InvalidParameterException (400) and ResourceNotFoundException (400); the recording shows no error at all — status, __type and Message all agree, only the staging label diverges.

Emulator: HTTP 200 with the same shape, but $.VersionIdsToStages.<caller-token-version>[0] is "AWSPREVIOUS" where AWS reports "AWSCURRENT" (replay detail: "$.VersionIdsToStages.sim2real-crt-b47f28475ba1da15589ab211f097c781[0]: value_mismatch (expected 'AWSCURRENT', got 'AWSPREVIOUS')"). The earlier PutSecretValue (task#4) and UpdateSecret (task#5) in the same fixture, both sent WITHOUT ClientRequestToken, were answered HTTP 200 instead of HTTP 400 InvalidRequestException; each stored a new version under a server-minted id and promoted it to AWSCURRENT, demoting the caller's version.

Why: AWSCURRENT is the label every GetSecretValue resolves when the caller names neither VersionId nor VersionStage. A client whose two writes AWS rejected, reading its secret back, is told its own version is no longer current — so it either reads a value AWS never stored or concludes its version was superseded. Rotation and promotion tooling decides what to move AWSPENDING/AWSCURRENT onto by reading this very map, so it acts on an inverted picture of which version is live.

Fix: the version/staging bookkeeping reached through the version-creating handlers (PutSecretValue and UpdateSecret) and the state that owns a secret's version-to-staging-label map — not DescribeSecret's serializer, which faithfully reports the labels it is handed. Hypothesis: the label is wrong purely because those handlers admit writes AWS refuses for a missing ClientRequestToken and default the version id instead of refusing; refusing at their request-validation seam leaves the caller's version holding AWSCURRENT and makes this step pass with no change to DescribeSecret.

Related open issues that do NOT own this replay key: 20261029T224105Z-secretsmanager-secrets-manager-accepts-createsecret-putsecretvalue-and-updatesecret-with.md (its "Cause 4 of 5"), 20261030T182552Z-secretsmanager-secrets-manager-describesecret-demotes-the-caller-s-own.md, 20261030T013256Z-secretsmanager-describesecret-reports-the-caller-s-own-clientrequesttoken-version.md and 20261030T123710Z-secretsmanager-secrets-manager-secret-version-staging-bookkeeping-puts-awscurrent.md all describe this same behavior, but their ownership keys embed the per-run ClientRequestToken value from an earlier replay (e.g. sim2real-crt-a1b2230e741ec402a10361dec5e8a2b9, sim2real-crt-feaa3781363869b83db3319ee3dc3d4a), while this run's key is sim2real-crt-b47f28475ba1da15589ab211f097c781[0] — so the keys cannot join and this cluster came back with an EMPTY owned_by. It is filed so the step is not left unowned and unreplayed; a fixer closing any of those issues should close this one with the same edit. The underlying key fragility is worth noting on its own: an ownership key that embeds a caller-supplied per-run token can never re-match a later replay.

Traces: secretsmanager-create-put-update-secret-without-client-request-token#task#6 (cluster of 1; affected_count 1).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up true, bootstrap_signal false; 155 steps, 135 passed, 16 mismatched, 4 blocked, score 87.1%, 9 divergence clusters).

Cluster: divergence_key "service=secretsmanager|DescribeSecret|status=200->200|keys=$.VersionIdsToStages.sim2real-crt-b47f28475ba1da15589ab211f097c781[0]", verdict mismatch, owned_by EMPTY, stub_path EMPTY.

Fixture: secretsmanager-create-put-update-secret-without-client-request-token, step task#6, operation DescribeSecret.
Expected status 200; actual status 200 — a body-value divergence only (no __type/Message difference).
Detail as reported: "$.VersionIdsToStages.sim2real-crt-b47f28475ba1da15589ab211f097c781[0]: value_mismatch (expected 'AWSCURRENT', got 'AWSPREVIOUS')".

Expected: the caller's own ClientRequestToken-keyed version mapped to ["AWSCURRENT"].
Actual: the same version mapped to ["AWSPREVIOUS"].

Mechanism visible in the preceding steps of the same fixture (all already owned by 20261029T224105Z-secretsmanager-secrets-manager-accepts-createsecret-putsecretvalue-and-updatesecret-with.md and NOT re-filed here): task#4 PutSecretValue and task#5 UpdateSecret, each sent with no ClientRequestToken, recorded HTTP 400 __type "InvalidRequestException" Message "You must provide a ClientRequestToken value. We recommend a UUID-type value." but were answered HTTP 200, minting a version apiece.

Model facts used (smithy_query on services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json): DescribeSecret output VersionIdsToStages -> SecretVersionsToStagesMapType (key SecretVersionIdType @length 32..64, value SecretVersionStagesType list @length 1..20); DescribeSecret errors InternalServiceError 500, InvalidParameterException 400, ResourceNotFoundException 400. PutSecretValue.ClientRequestToken and UpdateSecret.ClientRequestToken: ClientRequestTokenType, @idempotencyToken, @length 32..64; both operations model InvalidRequestException (400).

## Steps to Reproduce
1. Point a raw HTTP client (not an AWS SDK — SDKs auto-fill @idempotencyToken members, so this path cannot be reached through one) at the emulator's Secrets Manager endpoint, signing requests normally.
2. POST X-Amz-Target: secretsmanager.CreateSecret with {"Name": "<unique>", "SecretString": "v1", "ClientRequestToken": "<your own token, 32-64 chars>"}. The returned VersionId is your token value and it holds AWSCURRENT.
3. POST X-Amz-Target: secretsmanager.DescribeSecret {"SecretId": "<that secret>"} and confirm VersionIdsToStages maps your token to ["AWSCURRENT"].
4. POST X-Amz-Target: secretsmanager.PutSecretValue {"SecretId": "<that secret>", "SecretString": "v2"} with NO ClientRequestToken member. AWS answers HTTP 400 InvalidRequestException; the emulator answers HTTP 200 and mints a version.
5. POST X-Amz-Target: secretsmanager.UpdateSecret {"SecretId": "<that secret>", "SecretString": "v3"} with NO ClientRequestToken member. AWS answers HTTP 400 InvalidRequestException; the emulator answers HTTP 200 and mints another version.
6. POST X-Amz-Target: secretsmanager.DescribeSecret {"SecretId": "<that secret>"} again. AWS reports VersionIdsToStages[<your token>] == ["AWSCURRENT"] (steps 4 and 5 never happened, and no other version exists); the emulator reports ["AWSPREVIOUS"] for it and lists its own minted versions — the divergence (fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#6).
7. Re-replay that fixture step to confirm a fix; substitute your own ClientRequestToken value for the map key, which differs per run.
