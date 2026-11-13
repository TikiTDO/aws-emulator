Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-idp.cfn-log-delivery-configuration-round-trip-and-removed-on-update cloudformation__cognito-idp.cfn-user-pool-risk-configuration-attachment-round-trip-and-removed-on-update cloudformation__cognito-idp.cfn-user-pool-ui-customization-attachment-round-trip-and-removed-on-update cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-proves-and-removes-membership cloudformation__cognito-idp.cfn-log-delivery-configuration-user-auth-events-rejected-without-plus-tier cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-nonexistent-username-rejected-rollback cloudformation__cognito-idp.cfn-user-pool-properties-round-trip-and-removed-on-stack-delete
Issue-Key: cloudformation__cognito-idp.cfn-log-delivery-configuration-round-trip-and-removed-on-update|run#13|AssertionError|assert (out["LogDeliveryRef"], out["LogDeliveryId"]) == (pool_id, pool_id), (
Issue-Key: cloudformation__cognito-idp.cfn-user-pool-risk-configuration-attachment-round-trip-and-removed-on-update|run#11|AssertionError|assert out["RiskRef"] == f"{pool_id}|ALL", f"risk attachment Ref {out['RiskRef']!r}"
Issue-Key: cloudformation__cognito-idp.cfn-user-pool-ui-customization-attachment-round-trip-and-removed-on-update|run#17|AssertionError|assert out["UICRef"] == f"{pool_id}|{client_id}", f"UI customization Ref {out['UICRef']!r}"
Issue-Key: cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-proves-and-removes-membership|run#12|AssertionError|assert out["AttachRef"] == want_ref, f"attachment Ref {out['AttachRef']!r}, expected {want_ref!r}"
Issue-Key: cloudformation__cognito-idp.cfn-log-delivery-configuration-user-auth-events-rejected-without-plus-tier|run#15|AssertionError|assert config["LogConfigurations"] == want, f"control log configuration {config!r}"
Issue-Key: cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-nonexistent-username-rejected-rollback|run#5|AssertionError|assert status == "ROLLBACK_COMPLETE", f"nonexistent-username stack ended {status!r}"
Issue-Key: cloudformation__cognito-idp.cfn-user-pool-properties-round-trip-and-removed-on-stack-delete|run#22|AssertionError|assert issuer == getatt_provider_url, (
Issue-Commit: fea0c2e24ffe0fe10d2eaebdde89aa1a68933695

# cloudformation__cognito-idp: 4 net-new causes — Cognito attachment/configuration resources get generated physical ids instead of their composite Ref, LogDeliveryConfiguration is never applied to the pool, UserToGroupAttachment accepts a nonexistent Username, and GetAtt ProviderURL disagrees with the pool's own OIDC issuer

> _Note: this issue was identified against the code base at commit `fea0c2e24ffe0fe10d2eaebdde89aa1a68933695`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four distinct root causes in the CloudFormation providers for AWS::Cognito::* resources. Each is numbered with its own failing check ids; all four must be closed.

Cause 1 of 4: the physical resource id (and therefore Ref / Fn::GetAtt Id) of Cognito configuration-and-attachment resources is an auto-generated stack-scoped name instead of the composite id AWS assigns.
AWS: these resource types have no name of their own — their identity IS the keys that address them, and CloudFormation returns exactly those. AWS::Cognito::LogDeliveryConfiguration: Ref and Fn::GetAtt Id are the user pool id (the <region>_<suffix> value, i.e. the same value as the pool's Ref). AWS::Cognito::UserPoolRiskConfigurationAttachment: Ref is '<UserPoolId>|<ClientId>', and for the pool-wide attachment the ClientId component is the literal 'ALL'. AWS::Cognito::UserPoolUICustomizationAttachment: Ref is '<UserPoolId>|<ClientId>'. AWS::Cognito::UserPoolUserToGroupAttachment: Ref is '<UserPoolId>|<GroupName>|<Username>'. In every case the components are values taken from the template/request and from Cognito's own state — never a generated token. (Smithy: SetLogDeliveryConfiguration / SetRiskConfiguration / SetUICustomization / AdminAddUserToGroup are all keyed only by UserPoolId plus ClientId / GroupName+Username; there is no resource-name member to mint from.)
Emulator: each one gets '<stack-name>-<LogicalId>-<hex>' — e.g. LogDelivery Ref and GetAtt Id both '<stack>-LogDelivery-<hex>' where the pool id was expected; Risk Ref '<stack>-Risk-<hex>' where '<poolId>|ALL' was expected; UIC Ref '<stack>-UIC-<hex>' where '<poolId>|<clientId>' was expected; Attach Ref '<stack>-Attach-<hex>' where '<poolId>|<groupName>|<username>' was expected.
Why: Ref is the documented way to pass these resources' identity to another resource, an Output, or a consumer stack. Every template that does so wires a nonexistent identifier; nothing downstream can resolve the pool, client, group or user, and drift/update logic that re-addresses the resource by physical id addresses a token no Cognito API accepts.
Fix: the physical-resource-id assignment in the CloudFormation resource providers for these four AWS::Cognito::* types (the generated-name fallback must not be used for resources whose id is composite). Related open issue 20261111T111216Z-cloudformation-cognito-idp-aws-cognito-userpool-physical-resource-id-is-the.md reports the same defect class for AWS::Cognito::UserPool; it does not own these checks.
Traces: cloudformation__cognito-idp.cfn-log-delivery-configuration-round-trip-and-removed-on-update, cloudformation__cognito-idp.cfn-user-pool-risk-configuration-attachment-round-trip-and-removed-on-update, cloudformation__cognito-idp.cfn-user-pool-ui-customization-attachment-round-trip-and-removed-on-update, cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-proves-and-removes-membership.

Cause 2 of 4: AWS::Cognito::LogDeliveryConfiguration reaches CREATE_COMPLETE without writing anything to the pool.
AWS: creating that resource is SetLogDeliveryConfiguration on the named pool, so GetLogDeliveryConfiguration afterwards returns LogDeliveryConfiguration { UserPoolId: <pool id>, LogConfigurations: [ the template's entries ] } — here one entry with EventSource 'userNotification', LogLevel 'ERROR' and CloudWatchLogsConfiguration.LogGroupArn equal to the ARN the template built ('arn:<partition>:logs:<region>:<account>:log-group:<group name>'). Smithy: SetLogDeliveryConfiguration requires UserPoolId and LogConfigurations (list, length 0..2) and returns the stored LogDeliveryConfiguration; GetLogDeliveryConfiguration returns the same shape.
Emulator: the stack reports CREATE_COMPLETE but GetLogDeliveryConfiguration answers {'UserPoolId': '<pool id>', 'LogConfigurations': []} — the configuration was never applied. (Observed on the positive control of the tier check, which only reads the control pool back.)
Why: a stack that provisions audit/notification log delivery produces a pool with logging silently off. Nothing is delivered to the declared log group, and a client has no signal that its declared configuration was dropped.
Fix: the CloudFormation provider for AWS::Cognito::LogDeliveryConfiguration must call through to the cognito-idp SetLogDeliveryConfiguration state write (and its Delete/Update must clear or replace those entries) instead of only recording a resource row.
Traces: cloudformation__cognito-idp.cfn-log-delivery-configuration-user-auth-events-rejected-without-plus-tier (the same cause also blocks the read-back leg of cfn-log-delivery-configuration-round-trip-and-removed-on-update, which fails first on Cause 1).

Cause 3 of 4: AWS::Cognito::UserPoolUserToGroupAttachment naming a Username the pool never created succeeds, so the stack reaches CREATE_COMPLETE instead of rolling back.
AWS: the resource's create is AdminAddUserToGroup, which refuses an unknown user. Smithy (cognito-idp): AdminAddUserToGroup errors include UserNotFoundException (httpError 404) and ResourceNotFoundException (404). CloudFormation turns that refusal into a CREATE_FAILED on the attachment and, with rollback enabled, the stack ends ROLLBACK_COMPLETE and the pool the stack had created is gone (DescribeUserPool on it refuses ResourceNotFoundException with 'User pool <poolId> does not exist.'). The attachment's recorded ResourceStatusReason is the handler envelope relaying the fixed text 'User does not exist.' with the SDK suffix '(Service: CognitoIdentityProvider, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)' and HandlerErrorCode 'NotFound'; only the request id and attempt count are per-request.
Emulator: the stack ends CREATE_COMPLETE — the attachment was created for a username that does not exist, with no CREATE_FAILED event and no rollback.
Why: a template typo in Username is reported as a successful deploy, and the stack claims a group membership that does not exist. The negative class CloudFormation exists to surface (a resource an owning service refuses) is lost, and the rollback contract (the pool created by the failed stack must be gone) is never exercised.
Fix: the CloudFormation provider for AWS::Cognito::UserPoolUserToGroupAttachment must perform the real AdminAddUserToGroup and propagate its refusal; the cognito-idp AdminAddUserToGroup handler (.../src/ops/admin_add_user_to_group.rs) must refuse an unknown Username with UserNotFoundException 'User does not exist.' rather than accepting it.
Traces: cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-nonexistent-username-rejected-rollback.

Cause 4 of 4: Fn::GetAtt ProviderURL / ProviderName on AWS::Cognito::UserPool and the issuer in the pool's own OpenID-Connect discovery document name different hosts.
AWS: ProviderName is the pool's provider host-and-path and ProviderURL is 'https://' + ProviderName; the same string is the 'issuer' of the pool's /<poolId>/.well-known/openid-configuration document and the 'iss' claim of every token the pool issues. The requirement is that the three agree — the one address a client discovers from the stack output is the one the pool publishes and signs with. The host/port itself is environment-dependent and is not the requirement.
Emulator: GetAtt ProviderURL is the real-AWS form 'https://cognito-idp.<region>.amazonaws.com/<poolId>' (consistent with GetAtt ProviderName, which passes), while the discovery document the emulator serves for the same pool names issuer 'https://<emulator host>:<port>/<poolId>'. The two disagree, so one of them is unusable.
Why: a client takes ProviderURL from the stack output, appends /.well-known/openid-configuration or /.well-known/jwks.json, and validates tokens against it. Against the emulator that URL is a host nothing serves, while the issuer the pool actually mints is a different string, so issuer validation fails for every consumer configured from the stack output (OIDC clients, API Gateway/AppSync authorizers, identity-pool provider registration).
Fix: make the user-pool provider identity one value — the cognito-idp pool state's provider name/URL must be the same address the discovery document's issuer and the token 'iss' claim use, and the CloudFormation AWS::Cognito::UserPool provider must emit that value for GetAtt ProviderName/ProviderURL. Related open issues reporting the issuer form on the token side: 20261111T061507Z-cognito-idp-3-net-new-causes-the-id.md and 20261109T124858Z-cognito-idp-5-net-new-causes-tokens-are.md; neither owns this check.
Traces: cloudformation__cognito-idp.cfn-user-pool-properties-round-trip-and-removed-on-stack-delete.

## Evidence
Cause 1 — generated physical ids:
  cfn-log-delivery-configuration-round-trip-and-removed-on-update, line 198:
    assert (out["LogDeliveryRef"], out["LogDeliveryId"]) == (pool_id, pool_id)
    AssertionError: LogDeliveryConfiguration Ref '<stack>-LogDelivery-<hex>', GetAtt Id '<stack>-LogDelivery-<hex>', expected the listed pool Id 'us-west-2_<suffix>'
  cfn-user-pool-risk-configuration-attachment-round-trip-and-removed-on-update, line 179:
    assert out["RiskRef"] == f"{pool_id}|ALL"
    AssertionError: risk attachment Ref '<stack>-Risk-<hex>'
  cfn-user-pool-ui-customization-attachment-round-trip-and-removed-on-update, line 196:
    assert out["UICRef"] == f"{pool_id}|{client_id}"
    AssertionError: UI customization Ref '<stack>-UIC-<hex>'
  cfn-user-pool-user-to-group-attachment-proves-and-removes-membership, line 199:
    AssertionError: attachment Ref '<stack>-Attach-<hex>', expected '<poolId>|<groupName>|<username>'

Cause 2 — LogDeliveryConfiguration not applied:
  cfn-log-delivery-configuration-user-auth-events-rejected-without-plus-tier, line 213:
    assert config["LogConfigurations"] == want
    AssertionError: control log configuration {'UserPoolId': 'us-west-2_<suffix>', 'LogConfigurations': []}
  Expected: [{'EventSource': 'userNotification', 'LogLevel': 'ERROR', 'CloudWatchLogsConfiguration': {'LogGroupArn': 'arn:<partition>:logs:<region>:<account>:log-group:<group>'}}] — the stack reached CREATE_COMPLETE first.

Cause 3 — nonexistent Username accepted:
  cfn-user-pool-user-to-group-attachment-nonexistent-username-rejected-rollback, line 234:
    assert status == "ROLLBACK_COMPLETE"
    AssertionError: nonexistent-username stack ended 'CREATE_COMPLETE'
  Expected: ROLLBACK_COMPLETE, the Attach resource's reason relaying 'User does not exist. (Service: CognitoIdentityProvider, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)' with HandlerErrorCode 'NotFound', and the stack's pool gone (ResourceNotFoundException 'User pool <poolId> does not exist.').

Cause 4 — provider URL vs issuer:
  cfn-user-pool-properties-round-trip-and-removed-on-stack-delete, line 91:
    assert issuer == getatt_provider_url
    AssertionError: the pool's own OIDC discovery document names issuer 'https://<emulator-host>:<port>/us-west-2_<suffix>', which should match Fn::GetAtt ProviderURL 'https://cognito-idp.<region>.amazonaws.com/us-west-2_<suffix>'
  The preceding assertion (ProviderURL == 'https://' + ProviderName) passed, so the two GetAtts agree with each other and only the published issuer diverges.

## Steps to Reproduce
Cause 1: deploy a stack with AWS::Cognito::UserPool plus (a) AWS::Cognito::LogDeliveryConfiguration, (b) AWS::Cognito::UserPoolRiskConfigurationAttachment with ClientId 'ALL', (c) AWS::Cognito::UserPoolUICustomizationAttachment, (d) AWS::Cognito::UserPoolUserToGroupAttachment, and Output Ref (and Fn::GetAtt Id for the log-delivery resource) of each. Compare each Output against the pool id, '<poolId>|ALL', '<poolId>|<clientId>' and '<poolId>|<groupName>|<username>' respectively.
Cause 2: in the same stack declare AWS::Logs::LogGroup and an AWS::Cognito::LogDeliveryConfiguration with LogConfigurations=[{EventSource: userNotification, LogLevel: ERROR, CloudWatchLogsConfiguration: {LogGroupArn: <that group's ARN>}}]; after CREATE_COMPLETE call GetLogDeliveryConfiguration(UserPoolId=<pool>) and observe LogConfigurations: [].
Cause 3: deploy a stack with Pool + Group + User (Username 'u') + UserToGroupAttachment whose Username is 'u-missing' (DependsOn Group and User), rollback enabled; poll DescribeStacks to a terminal status and observe CREATE_COMPLETE instead of ROLLBACK_COMPLETE; then AdminListGroupsForUser shows a membership for a user that does not exist.
Cause 4: deploy a stack with one AWS::Cognito::UserPool and Outputs Fn::GetAtt ProviderName / ProviderURL; GET <cognito-idp endpoint>/<poolId>/.well-known/openid-configuration and compare its 'issuer' with the ProviderURL output.
