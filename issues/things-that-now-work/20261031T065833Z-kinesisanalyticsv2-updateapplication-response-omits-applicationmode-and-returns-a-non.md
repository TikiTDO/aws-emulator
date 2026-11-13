Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-application-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-application-lifecycle kinesisanalyticsv2-application-version-history
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|UpdateApplication|2xx->2xx|$.ApplicationDetail.ApplicationMode
Issue-Key: kinesisanalyticsv2|UpdateApplication|2xx->2xx|$.ApplicationDetail.ConditionalToken
Issue-Commit: 7846fec8a86647e77da10e6415de80579a2805a4

# UpdateApplication response omits ApplicationMode and returns a non-conforming ConditionalToken

> _Note: this issue was identified against the code base at commit `7846fec8a86647e77da10e6415de80579a2805a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApplication

## Description
Shared fault: the ApplicationDetail the UpdateApplication handler serializes on the mutation (write) path is built without the application's mode and without a freshly-minted concurrency token, so the response an accepted update returns is not the detail AWS returns. 1 cause.

Cause 1 of 1: UpdateApplication returns 200 with $.ApplicationDetail.ApplicationMode absent and a ConditionalToken whose shape is not the modeled/recorded one.
AWS: Smithy (com.amazonaws.kinesisanalyticsv2) UpdateApplication output member ApplicationDetail -> ApplicationDetail is @required. ApplicationDetail declares ApplicationMode (enum ApplicationMode, values STREAMING | INTERACTIVE; doc: "To create a Managed Service for Apache Flink Studio notebook, you must set the mode to INTERACTIVE. However, for a Managed Service for Apache Flink application, the mode is optional.") and ConditionalToken (string, @length 1..512, @pattern ^[a-zA-Z0-9-_+/=]+$; doc: "A value you use to implement strong concurrency for application updates."). Recorded behavior: for a non-notebook FLINK application, every ApplicationDetail AWS returns - including the one UpdateApplication echoes - carries ApplicationMode STREAMING, and carries a ConditionalToken whose value the fixture asserts only by shape (an opaque token, not a hyphenated 36-char UUID) and which is re-issued as the application version advances. The mode value tracks the application (STREAMING for a streaming application, INTERACTIVE for a Studio notebook); the token's concrete characters are per-environment, its shape family and its rotation per accepted update are the requirement.
Emulator: answers 200 (status matches) but $.ApplicationDetail.ApplicationMode is absent (null) where STREAMING is expected, and $.ApplicationDetail.ConditionalToken is emitted as a hyphenated 36-character UUID that the fixture's shape assertion rejects (observed '1ccd74b8-fdaa-f614-87c2-b04525e005f6' on kinesisanalyticsv2-application-lifecycle#task#4 and the same constant value repeated across the two consecutive updates in kinesisanalyticsv2-application-version-history, i.e. it is not re-issued per update). Hypothesis: AWS issues an opaque base64-style token rather than a UUID; the modeled pattern itself permits hyphens, so only the shape family and the rotation differ.
Why: a client that calls UpdateApplication and reads the returned detail - the normal way to avoid an extra DescribeApplication - sees applicationMode() == null and either NPEs or concludes it cannot tell a STREAMING application from an INTERACTIVE notebook. Worse for concurrency: ConditionalToken is this API's strong-concurrency handle, so a client that takes the token from the update response and feeds it to the next UpdateApplication is handed a token that never changed; token-based optimistic concurrency can never detect a conflicting concurrent update, and conditional-update failure paths are never exercised.
Fix: the UpdateApplication handler's ApplicationDetail serializer must emit ApplicationMode from the stored application, and the application state's update path must mint a new ConditionalToken (in the modeled opaque-token shape, @pattern ^[a-zA-Z0-9-_+/=]+$, length 1..512) on each accepted update instead of reusing the UUID generated at create time.
Traces: kinesisanalyticsv2-application-lifecycle#task#4 (representative); also kinesisanalyticsv2-application-version-history#task#4 and kinesisanalyticsv2-application-version-history#task#8 (same cluster).

Note on overlap with open issues: 20261029T173921Z-kinesisanalyticsv2-applicationdetail-and-applicationsummary-omit-modeled-description-members-applicationmode.md covers the same omissions on the READ paths (CreateApplication/DescribeApplication/ListApplications) and does not carry this cluster's replay key; 20261029T173921Z-kinesisanalyticsv2-deleteapplication-skips-createtimestamp-deserialization-and-updateapplication-never-advances.md covers UpdateApplication not advancing ApplicationVersionId / not applying the configuration, a different defect from the response serializer gap filed here. Neither owns kinesisanalyticsv2-application-lifecycle#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (summary: 55 steps, 21 passed, 34 mismatched, score 38.2%, 10 divergence clusters, all kinesisanalyticsv2).

Cause 1 - divergence key service=kinesisanalyticsv2|UpdateApplication|status=200->200|keys=$.ApplicationDetail.ApplicationMode,$.ApplicationDetail.ConditionalToken; verdict mismatch; expected_status 200, actual_status 200; cluster size 3.
Representative step kinesisanalyticsv2-application-lifecycle#task#4 (UpdateApplication):
  $.ApplicationDetail.ApplicationMode: missing (expected 'STREAMING', got None)
  $.ApplicationDetail.ConditionalToken: value_mismatch (expected '<shaped-not-asserted>', got '1ccd74b8-fdaa-f614-87c2-b04525e005f6')
Other affected steps in the same cluster: kinesisanalyticsv2-application-version-history#task#4, kinesisanalyticsv2-application-version-history#task#8 - the latter two being consecutive updates of one application that both return the identical ConditionalToken, showing the token is not re-issued per accepted update.

Model evidence: smithy_query operation=UpdateApplication -> output {ApplicationDetail (required) -> ApplicationDetail, OperationId}. smithy_query shape=ApplicationDetail -> member ApplicationMode : enum ApplicationMode (values STREAMING, INTERACTIVE); member ConditionalToken : string @length 1..512 @pattern ^[a-zA-Z0-9-_+/=]+$ ("A value you use to implement strong concurrency for application updates.").

## Steps to Reproduce
1. Clone this checkout, boot the emulator from it, and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture kinesisanalyticsv2-application-lifecycle through step task#4 - an UpdateApplication on the FLINK-1_15 STREAMING application created at task#2 - and diff the returned ApplicationDetail against the recorded response. Expect ApplicationMode 'STREAMING' present; the emulator omits the key entirely.
3. In the same response inspect ApplicationDetail.ConditionalToken: it is a hyphenated 36-character UUID, which the fixture's shape assertion for an opaque token rejects.
4. Replay fixture kinesisanalyticsv2-application-version-history through its two UpdateApplication steps task#4 and task#8 and compare the ConditionalToken values of the two responses: they are identical, so no new token was issued for the second accepted update.
