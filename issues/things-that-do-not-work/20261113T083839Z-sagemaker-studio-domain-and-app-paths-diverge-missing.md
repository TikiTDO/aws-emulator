Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-domain-app-presigned-url-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-domain-app-presigned-url-error-paths sagemaker-studio-domain-app-lifecycle
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreateDomain|4xx->2xx|$.Url
Issue-Key: sagemaker|CreateDomain|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateDomain|4xx->2xx|$.message
Issue-Key: sagemaker|CreateDomain|4xx->2xx|status
Issue-Key: sagemaker|CreateDomain|2xx->2xx|$.Url
Issue-Key: sagemaker|DescribeDomain|2xx->2xx|$.AppNetworkAccess
Issue-Key: sagemaker|DescribeDomain|2xx->2xx|$.DefaultUserSettings.AutoMountHomeEFS
Issue-Key: sagemaker|DescribeDomain|2xx->2xx|$.DefaultUserSettings.DefaultLandingUri
Issue-Key: sagemaker|DescribeDomain|2xx->2xx|$.DefaultUserSettings.StudioWebPortal
Issue-Key: sagemaker|DescribeDomain|2xx->2xx|$.DefaultUserSettings.StudioWebPortalSettings
Issue-Key: sagemaker|DescribeDomain|2xx->2xx|$.Url
Issue-Key: sagemaker|CreateApp|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateApp|4xx->2xx|$.message
Issue-Key: sagemaker|CreateApp|4xx->2xx|status
Issue-Commit: fae38c34e99cb924099b47e9ab649e154c215f8c

# sagemaker Studio domain and app paths diverge: missing create-time validation, omitted DescribeDomain settings, non-served Studio Url

> _Note: this issue was identified against the code base at commit `fae38c34e99cb924099b47e9ab649e154c215f8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
FOUR net-new causes in the SageMaker AI Studio domain/app surface. All four must be closed before this issue is submitted.

Cause 1 of 4: CreateDomain accepts a domain request that carries neither VpcId nor SubnetIds.
AWS: CreateDomain's input models SubnetIds (Subnets, @length 1..16) and VpcId (@length 0..32, pattern ^[-0-9a-zA-Z]+$) as optional members, but a domain that is not created by SageMaker Unified Studio must supply both; AWS refuses the request before any domain is created. Recorded response: HTTP 400 with __type 'ValidationException' and message 'VpcId and SubnetIds are required for SageMaker AI Domain not created by SageMaker Unified Studio.' The whole message is fixed text for this error class — it interpolates nothing from the request. (Smithy: CreateDomain input members SubnetIds/VpcId both clientOptional; modeled errors are only ResourceInUse and ResourceLimitExceeded, so ValidationException comes from the service-common error set — recorded behavior for the message.)
Emulator: answers HTTP 200 with a CreateDomainResponse — no __type, no message — and even hands back a $.Url presigned Studio URL, i.e. the domain is created.
Why: a client that relies on AWS refusing an under-specified domain gets a 'successful' domain it never asked for. Every later step in the scenario (user profile, app, presigned URL) then operates on a domain whose networking was never resolved, so the negative test passes on AWS and silently mutates state here; error-handling code paths for ValidationException are never exercised.
Fix: CreateDomain's input validation in the sagemaker service crate — add the 'neither VpcId nor SubnetIds supplied' precondition ahead of domain creation and raise ValidationException/400 with the fixed sentence above.
Traces: sagemaker-domain-app-presigned-url-error-paths#task#1.

Cause 2 of 4: CreateDomain publishes a Studio Url on a host the emulator does not serve.
AWS: CreateDomain output member Url (String1024) is the domain's Studio landing URL; on real AWS that host is served by AWS (Smithy: CreateDomain output Url -> String1024 @length 0..1024). The recording asserts only the URL's shape, not its literal value, so the host is not itself a requirement — what IS a requirement is that the announced address is reachable at the endpoint the caller is talking to.
Emulator: emits a Url whose host is a real-AWS Studio DNS name of the form https://d-<domainId>.studio.<region>.sagemaker.aws, an address the emulator does not own or serve; the replay classifies it host_not_own. (Hypothesis: the Url is formatted from a hard-coded AWS template rather than from the emulator's own advertised endpoint.)
Why: any client that follows the returned Url leaves the emulator entirely — the request either escapes to real AWS or fails name resolution. A presigned-URL/Studio flow can therefore never be driven end to end against the emulator.
Fix: the CreateDomain handler's Url construction (and the shared Studio-URL formatter it uses) must derive host and port from the emulator's own serving endpoint instead of the AWS DNS template.
Traces: sagemaker-studio-domain-app-lifecycle#setup#6 (and the same Url surfaces in DescribeDomain at sagemaker-studio-domain-app-lifecycle#task#7).

Cause 3 of 4: DescribeDomain omits the domain's resolved default settings.
AWS: DescribeDomain echoes the stored domain configuration, including DefaultUserSettings (UserSettings) with its AutoMountHomeEFS (enum), DefaultLandingUri (LandingUri @length 0..1023, documented value 'studio::' when Studio is the default experience), StudioWebPortal (enum ENABLED/DISABLED) and StudioWebPortalSettings.ExecutionRoleSessionNameMode (enum, USER_IDENTITY) members, plus the domain's app network access member — the recording shows it as $.AppNetworkAccess with value 'DirectInternetOnly' (recorded behavior only: the on-disk model's DescribeDomainResponse carries AppNetworkAccessType with values PublicInternetOnly/VpcOnly and no AppNetworkAccess member). The requirement is that these resolved members are present with the values the domain actually carries, not the literal values from one recording.
Emulator: returns 200 but every one of those members is absent: $.AppNetworkAccess, $.DefaultUserSettings.AutoMountHomeEFS, $.DefaultUserSettings.DefaultLandingUri, $.DefaultUserSettings.StudioWebPortal and $.DefaultUserSettings.StudioWebPortalSettings all come back None.
Why: a client reading back a domain cannot tell how the domain was resolved — whether Studio is reachable, which landing experience applies, whether home-EFS auto-mount is on, or how execution-role session names are derived. Round-tripping a domain through Describe and re-applying it loses settings, and tooling that branches on StudioWebPortal/DefaultLandingUri takes the wrong branch.
Fix: the DescribeDomain serializer plus the domain state record in the sagemaker crate — persist and default these UserSettings members at CreateDomain time and emit them on describe.
Traces: sagemaker-studio-domain-app-lifecycle#task#7.

Cause 4 of 4: CreateApp creates an app for a user profile that does not exist.
AWS: CreateApp takes DomainId (required) and an optional UserProfileName (@length 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$); when the named user profile is not present in the domain AWS refuses the call rather than creating the app. Recorded response: HTTP 400, __type 'ValidationException', message 'UserProfile [<UserProfileName>] does not exist.' — fixed text 'UserProfile [' ... '] does not exist.' with the request's UserProfileName as the only variable part. (Smithy: CreateApp models only ResourceInUse and ResourceLimitExceeded, so ValidationException is from the service-common set — recorded behavior for the message.)
Emulator: answers HTTP 200 with a CreateAppResponse and no error members at all, so a dangling app is created against a non-existent user profile.
Why: the referential check a client depends on is absent — apps accumulate under principals that were never created, and the modeled 400 path is unreachable, so client retry/repair logic for a mistyped or deleted user profile never runs.
Fix: CreateApp's validation in the sagemaker crate — resolve UserProfileName against the domain's user-profile state before allocating the app and raise ValidationException/400 with the sentence above.
Traces: sagemaker-studio-domain-app-lifecycle#task#8.

Related open issues that do NOT cover these causes: '20261109T134927Z-cloudformation-ec2-iam-sagemaker-cloudformation-sagemaker-domain-space-stacks-2-net-new.md' concerns CloudFormation AWS::SageMaker::Domain/Space resource handling, not the direct CreateDomain/CreateApp/DescribeDomain wire paths above.

## Evidence
Cause 1 — sagemaker-domain-app-presigned-url-error-paths#task#1, CreateDomain: expected status 400, got 200; $.__type expected 'ValidationException', got None; $.message expected 'VpcId and SubnetIds are required for SageMaker AI Domain not created by SageMaker Unified Studio.', got None; $.Url expected None, got a Studio URL (host_not_own).
Cause 2 — sagemaker-studio-domain-app-lifecycle#setup#6, CreateDomain 200->200: $.Url host_not_own (expected shape-only, got 'https://d-<domainId>.studio.us-west-2.sagemaker.aws', a host the emulator does not serve).
Cause 3 — sagemaker-studio-domain-app-lifecycle#task#7, DescribeDomain 200->200: $.AppNetworkAccess missing (expected 'DirectInternetOnly'); $.DefaultUserSettings.AutoMountHomeEFS missing (expected 'Enabled'); $.DefaultUserSettings.DefaultLandingUri missing (expected 'studio::'); $.DefaultUserSettings.StudioWebPortal missing (expected 'ENABLED'); $.DefaultUserSettings.StudioWebPortalSettings missing (expected {'ExecutionRoleSessionNameMode': 'USER_IDENTITY'}); $.Url host_not_own.
Cause 4 — sagemaker-studio-domain-app-lifecycle#task#8, CreateApp: expected status 400, got 200; $.__type expected 'ValidationException', got None; $.message expected 'UserProfile [<UserProfileName>] does not exist.', got None.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture sagemaker-domain-app-presigned-url-error-paths, step task#1 — CreateDomain with AuthMode/DomainName/DefaultUserSettings but no VpcId and no SubnetIds; observe 200 instead of 400 ValidationException.
3. Cause 2: replay fixture sagemaker-studio-domain-app-lifecycle, step setup#6 — CreateDomain; inspect the returned $.Url host and compare it to the emulator's own data endpoint.
4. Cause 3: replay fixture sagemaker-studio-domain-app-lifecycle, step task#7 — DescribeDomain on the domain created in setup; observe the absent AppNetworkAccess and DefaultUserSettings members.
5. Cause 4: replay fixture sagemaker-studio-domain-app-lifecycle, step task#8 — CreateApp naming a UserProfileName that was never created in the domain; observe 200 instead of 400 ValidationException.
