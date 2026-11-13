Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cfn-cognito-userpool-lambda-trigger-config-positive
Issue-Key: apigateway__cognito-idp__lambda.cfn-cognito-userpool-lambda-trigger-config-positive|run#9|AssertionError|assert signup["UserConfirmed"] is True, (
Issue-Commit: fea0c2e24ffe0fe10d2eaebdde89aa1a68933695

# A CloudFormation-wired UserPool LambdaConfig.PreSignUp trigger is never invoked: SignUp returns UserConfirmed=false and the user stays UNCONFIRMED

> _Note: this issue was identified against the code base at commit `fea0c2e24ffe0fe10d2eaebdde89aa1a68933695`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignUp

## Description
Cause 1 of 1: a user pool whose LambdaConfig.PreSignUp was set by CloudFormation to a function ARN in the same stack does not invoke that function on SignUp, so the trigger's response mutation is lost.

AWS: a user pool carrying LambdaConfig.PreSignUp invokes that Lambda synchronously as part of SignUp, with triggerSource PreSignUp_SignUp, and honours the mutable fields the function returns in event.response. A handler that sets response.autoConfirmUser = true makes SignUp answer UserConfirmed: true and leaves the user in UserStatus CONFIRMED (AdminGetUser), with no confirmation code required. Smithy (cognito-idp): SignUp output member UserConfirmed is the pool's answer about whether confirmation is still needed; CreateUserPool/UpdateUserPool accept LambdaConfig.PreSignUp, and InitiateAuth/SignUp model UserLambdaValidationException, InvalidLambdaResponseException and UnexpectedLambdaException precisely because the trigger is executed inline. The pool, the function and the trigger wiring here all come from one CloudFormation stack (LambdaConfig: { PreSignUp: Fn::GetAtt [PreSignUpFn, Arn] }).

Emulator: SignUp answers HTTP 200 with {'UserConfirmed': False, 'UserSub': '<uuid>'} — the trigger did not run (or the ARN CloudFormation wrote onto the pool is not the one the pool invokes). No trigger error is reported either, so the call looks like a pool with no PreSignUp trigger at all.

Why: every pre-sign-up policy a client expresses in Lambda — auto-confirm, auto-verify, domain allow-listing, blocking a sign-up by raising — has no effect. Sign-ups that AWS auto-confirms stay UNCONFIRMED, so the following sign-in fails with UserNotConfirmedException, and sign-ups a trigger is supposed to reject are accepted. A CloudFormation-declared trigger is silently inert, which also means the deploy-then-use pattern cannot be tested at all.

Fix: the cognito-idp SignUp handler (.../src/ops/sign_up.rs) and the user-pool trigger-dispatch component that resolves LambdaConfig and invokes the function, plus the CloudFormation provider for AWS::Cognito::UserPool which must persist LambdaConfig.PreSignUp onto the created pool in a form the dispatcher resolves. The response contract to apply is event.response.autoConfirmUser / autoVerifyEmail / autoVerifyPhone.

Related (does NOT own this check, per the replay report): 20261023T114649Z-cognito-idp-cognito-user-pool-lambda-triggers-are-not-invoked.md and 20261029T182116Z-apigateway-cognito-idp-lambda-execute-api-invocations-answer.md both describe trigger invocation failing on other paths; this check was not attributed to either and is reported here with its own replay key.

Traces: apigateway__cognito-idp__lambda.cfn-cognito-userpool-lambda-trigger-config-positive.

## Evidence
Check apigateway__cognito-idp__lambda.cfn-cognito-userpool-lambda-trigger-config-positive, verdict [mismatch]:

  File "check_apigateway__cognito_idp__lambda_cfn_cognito_userpool_lambda_trigger_config_positive.py", line 31, in run
    assert signup["UserConfirmed"] is True, (...)
  AssertionError: SignUp did not come back auto-confirmed -- the PreSignUp trigger did not run (or was not the one configured on this pool): {'UserConfirmed': False, 'UserSub': '<uuid>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Expected (AWS): SignUp -> UserConfirmed: true, then AdminGetUser -> UserStatus 'CONFIRMED', because the stack's PreSignUp handler sets event['response']['autoConfirmUser'] = True.
Actual (emulator): SignUp -> 200 with UserConfirmed: false; no trigger invocation observable, no trigger-related error raised.

## Steps to Reproduce
1. CreateStack with a template declaring: an AWS::IAM::Role trusted by lambda.amazonaws.com; an AWS::Lambda::Function (python3.12, inline ZipFile) whose handler sets event['response']['autoConfirmUser'] = True and returns event; an AWS::Cognito::UserPool with LambdaConfig: { PreSignUp: Fn::GetAtt [PreSignUpFn, Arn] }; and an app client. Wait for CREATE_COMPLETE.
2. Call SignUp(ClientId=<stack client id>, Username=<new user>, Password=<policy-compliant password>).
3. Observe UserConfirmed: false. AWS returns UserConfirmed: true.
4. Call AdminGetUser for that username: the emulator reports UserStatus UNCONFIRMED; AWS reports CONFIRMED.
