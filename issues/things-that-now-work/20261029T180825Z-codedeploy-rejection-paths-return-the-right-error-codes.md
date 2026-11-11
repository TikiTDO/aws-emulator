Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.create-deployment-config-invalid-min-healthy-rejected codedeploy.delete-predefined-deployment-config-fails codedeploy.predefined-deployment-config-delete-rejected codedeploy.create-deployment-group-conflicting-tag-filters-rejected codedeploy.deployment-group-ec2-tag-filters-and-tag-set-conflict codedeploy.on-premises-iam-user-arn-reuse-rejected codedeploy.register-onprem-instance-duplicate-iam-user-arn-rejected codedeploy.register-onprem-instance-duplicate-iam-session-arn-rejected codedeploy.put-lifecycle-event-hook-status-deployment-not-exist-rejected codedeploy.tag-resource-invalid-arn-rejected codedeploy.stop-deployment-already-completed-blocked
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.create-deployment-config-invalid-min-healthy-rejected|run#10|AssertionError|assert message == (
Issue-Key: codedeploy.delete-predefined-deployment-config-fails|run#12|AssertionError|assert error.get("Message") == "Can not delete a predefined deployment configuration", error
Issue-Key: codedeploy.predefined-deployment-config-delete-rejected|run#8|AssertionError|assert refused == (
Issue-Key: codedeploy.create-deployment-group-conflicting-tag-filters-rejected|run#16|AssertionError|assert message == (
Issue-Key: codedeploy.deployment-group-ec2-tag-filters-and-tag-set-conflict|run#15|AssertionError|assert message == (
Issue-Key: codedeploy.on-premises-iam-user-arn-reuse-rejected|run#17|AssertionError|assert message == (
Issue-Key: codedeploy.register-onprem-instance-duplicate-iam-user-arn-rejected|run#11|AssertionError|assert error == (
Issue-Key: codedeploy.register-onprem-instance-duplicate-iam-session-arn-rejected|run#11|AssertionError|assert error == (
Issue-Key: codedeploy.put-lifecycle-event-hook-status-deployment-not-exist-rejected|run#12|AssertionError|assert message2 == "The lifecycleEventHookExecutionId is invalid", f"got {message2!r}"
Issue-Key: codedeploy.tag-resource-invalid-arn-rejected|run#9|AssertionError|assert error == ("InvalidArnException", "Invalid ARN: not-a-valid-arn"), (
Issue-Key: codedeploy.stop-deployment-already-completed-blocked|run#26|AssertionError|assert terminal.get("errorInformation") == {
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# CodeDeploy rejection paths return the right error codes with the wrong error messages (9 causes)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeploymentConfig

## Description
Nine CodeDeploy refusal paths answer with the correct HTTP 400 and the correct modeled error code, but with an emulator-authored message instead of the message AWS words for that error class. The messages are built by interpolating request/state values (ARNs, names, ids, values) into locally invented sentences, where AWS's text for these errors is fixed (or fixed with a single documented variable). Nine independent message sites, nine causes.

Cause 1 of 9: CreateDeploymentConfig InvalidMinimumHealthyHostValueException message.
AWS: Smithy CreateDeploymentConfig models InvalidMinimumHealthyHostValueException (HTTP 400); recorded behavior: minimumHealthyHosts={type: FLEET_PERCENT, value: <out of 0..100>} is refused with message "The value for the minimum healthy hosts with type of FLEET_PERCENT should be positive and less than 100" — fixed text whose only variable part is the type name taken from the request's minimumHealthyHosts.type.
Emulator: same code, message "minimumHealthyHosts value -5 is not a percentage: a FLEET_PERCENT value must be between 0 and 100" (interpolates the rejected value, which AWS's message never carries).
Why: a client (or a test fixture) matching on AWS's refusal text does not recognize the refusal, and operators debugging a config see wording no AWS runbook contains.
Fix: CreateDeploymentConfig's input validation / error construction in the codedeploy service crate.
Traces: codedeploy.create-deployment-config-invalid-min-healthy-rejected.

Cause 2 of 9: DeleteDeploymentConfig InvalidOperationException message for a predefined CodeDeployDefault.* config.
AWS: Smithy DeleteDeploymentConfig models InvalidOperationException (HTTP 400); recorded behavior: the message is entirely fixed text with no variable parts — "Can not delete a predefined deployment configuration".
Emulator: same code, message "The deployment configuration CodeDeployDefault.OneAtATime is a predefined deployment configuration and cannot be deleted" (interpolates the config name).
Why: two separate checks assert the AWS text; a client branching on the refusal message for built-in configs misreads it.
Fix: DeleteDeploymentConfig's predefined-config guard / error construction.
Traces: codedeploy.delete-predefined-deployment-config-fails, codedeploy.predefined-deployment-config-delete-rejected.

Cause 3 of 9: CreateDeploymentGroup InvalidOnPremisesTagCombinationException message.
AWS: Smithy CreateDeploymentGroup models InvalidOnPremisesTagCombinationException (HTTP 400) for a request supplying both onPremisesInstanceTagFilters and onPremisesTagSet; recorded message is fixed text with no request values: "The request specified both OnPremisesTagFilters and OnPremisesTagSet, but only one of these data types can be used in a single call. Remove one tag specification type, and then try again."
Emulator: same code, message "Only one of onPremisesInstanceTagFilters and onPremisesTagSet can be used in a single call".
Why: the remediation sentence AWS gives ("Remove one tag specification type, and then try again.") is absent, and the wording differs from what clients and docs quote.
Fix: CreateDeploymentGroup's mutual-exclusion validation / error construction.
Traces: codedeploy.create-deployment-group-conflicting-tag-filters-rejected.

Cause 4 of 9: CreateDeploymentGroup InvalidEC2TagCombinationException message.
AWS: Smithy CreateDeploymentGroup models InvalidEC2TagCombinationException (HTTP 400) for a request supplying both ec2TagFilters and ec2TagSet; recorded message is fixed text: "The request specified both Ec2TagFilters and Ec2TagSet, but only one of these data types can be used in a single call. Remove one tag specification type, and then try again."
Emulator: same code, message "Only one of ec2TagFilters and ec2TagSet can be used in a single call".
Why: same as Cause 3 on the EC2 selector pair — the diverging text is the only thing a caller can read, and it is a requirement.
Fix: CreateDeploymentGroup's mutual-exclusion validation / error construction.
Traces: codedeploy.deployment-group-ec2-tag-filters-and-tag-set-conflict.

Cause 5 of 9: RegisterOnPremisesInstance IamUserArnAlreadyRegisteredException message.
AWS: Smithy RegisterOnPremisesInstance models IamUserArnAlreadyRegisteredException (HTTP 400); recorded message is entirely fixed text and names neither the ARN nor the owning instance: "The on-premises instance could not be registered because the request included an IAM user ARN that has already been used to register an instance. Include either a different IAM user ARN or IAM session ARN in the request, and then try again."
Emulator: same code, message "The user ARN <iamUserArn from the request> is already registered with the on-premises instance <existing instance name from state>".
Why: the emulator leaks state (which instance holds the binding) in a message AWS keeps generic, and no part of AWS's required text is present.
Fix: RegisterOnPremisesInstance's uniqueness guard / error construction.
Traces: codedeploy.on-premises-iam-user-arn-reuse-rejected, codedeploy.register-onprem-instance-duplicate-iam-user-arn-rejected.

Cause 6 of 9: RegisterOnPremisesInstance IamSessionArnAlreadyRegisteredException message.
AWS: Smithy RegisterOnPremisesInstance models IamSessionArnAlreadyRegisteredException (HTTP 400); recorded message is fixed text: "The on-premises instance could not be registered because the request included an IAM session ARN that has already been used to register a different instance. Include either a different IAM session ARN or IAM user ARN in the request, and then try again."
Emulator: same code, message "The IAM session ARN <iamSessionArn from the request> has already been used to register the on-premises instance <existing instance name from state>".
Why: same as Cause 5 for the session-ARN uniqueness constraint.
Fix: RegisterOnPremisesInstance's uniqueness guard / error construction.
Traces: codedeploy.register-onprem-instance-duplicate-iam-session-arn-rejected.

Cause 7 of 9: PutLifecycleEventHookExecutionStatus InvalidLifecycleEventHookExecutionIdException message.
AWS: Smithy PutLifecycleEventHookExecutionStatus models InvalidLifecycleEventHookExecutionIdException (HTTP 400); recorded message is fixed text with no variables: "The lifecycleEventHookExecutionId is invalid".
Emulator: same code, message "The lifecycleEventHookExecutionId <hook id from the request> is invalid: it names no lifecycle event hook execution of deployment <deploymentId from the request>".
Why: hook Lambdas and deployment tooling discriminate this error from DeploymentDoesNotExistException by code and message; the embellished text breaks any matcher written against AWS.
Fix: PutLifecycleEventHookExecutionStatus's hook-execution-id lookup / error construction.
Traces: codedeploy.put-lifecycle-event-hook-status-deployment-not-exist-rejected.

Cause 8 of 9: TagResource InvalidArnException message.
AWS: Smithy TagResource models InvalidArnException (HTTP 400) and declares ResourceArn required; recorded message is the fixed prefix "Invalid ARN: " followed by the offending value taken verbatim from the request — e.g. "Invalid ARN: <ResourceArn>".
Emulator: same code, message "The ARN <ResourceArn> is not in a valid format" — the fixed prefix AWS uses is absent.
Why: tagging helpers that surface or match the AWS prefix report an unrecognized failure.
Fix: TagResource's ARN parse / error construction.
Traces: codedeploy.tag-resource-invalid-arn-rejected.

Cause 9 of 9: deployment errorInformation message for the NO_INSTANCES failure.
AWS: recorded behavior: a Server deployment whose group's tag filters match no instance terminates Failed with errorInformation.code 'NO_INSTANCES' and the fixed message "The deployment failed because no instances were found for your deployment group. Check your deployment group settings to make sure the tags for your Amazon EC2 instances or Auto Scaling groups correctly identify the instances you want to deploy to, and then try again." (no request or state values appear in it).
Emulator: correct code 'NO_INSTANCES', message "No instances found for the deployment group of this deployment".
Why: errorInformation.message is what deployment dashboards and CI logs print for the single most common deployment failure; the wording is a requirement.
Fix: the deployment state machine's terminal-failure construction (errorInformation for NO_INSTANCES).
Traces: codedeploy.stop-deployment-already-completed-blocked.

Related open issue that does NOT cover these sites: "CodeDeploy accepts invalid input and misclassifies rejections" (wire) is about missing gates and wrong CODES, not these message texts.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against this checkout (89/135 passed; emulator serving, bootstrap_signal=false). Expected = AWS behavior asserted by the check, Actual = emulator response.

1. codedeploy.create-deployment-config-invalid-min-healthy-rejected — CreateDeploymentConfig(minimumHealthyHosts={type:FLEET_PERCENT,value:-5}): expected code InvalidMinimumHealthyHostValueException + message 'The value for the minimum healthy hosts with type of FLEET_PERCENT should be positive and less than 100'; got the same code with 'minimumHealthyHosts value -5 is not a percentage: a FLEET_PERCENT value must be between 0 and 100'.
2. codedeploy.delete-predefined-deployment-config-fails / codedeploy.predefined-deployment-config-delete-rejected — DeleteDeploymentConfig('CodeDeployDefault.OneAtATime'): expected ('InvalidOperationException', 'Can not delete a predefined deployment configuration'); got ('InvalidOperationException', 'The deployment configuration CodeDeployDefault.OneAtATime is a predefined deployment configuration and cannot be deleted').
3. codedeploy.create-deployment-group-conflicting-tag-filters-rejected — expected InvalidOnPremisesTagCombinationException + 'The request specified both OnPremisesTagFilters and OnPremisesTagSet, but only one of these data types can be used in a single call. Remove one tag specification type, and then try again.'; got 'Only one of onPremisesInstanceTagFilters and onPremisesTagSet can be used in a single call'.
4. codedeploy.deployment-group-ec2-tag-filters-and-tag-set-conflict — expected InvalidEC2TagCombinationException + 'The request specified both Ec2TagFilters and Ec2TagSet, but only one of these data types can be used in a single call. Remove one tag specification type, and then try again.'; got 'Only one of ec2TagFilters and ec2TagSet can be used in a single call'.
5. codedeploy.on-premises-iam-user-arn-reuse-rejected / codedeploy.register-onprem-instance-duplicate-iam-user-arn-rejected — expected IamUserArnAlreadyRegisteredException + 'The on-premises instance could not be registered because the request included an IAM user ARN that has already been used to register an instance. Include either a different IAM user ARN or IAM session ARN in the request, and then try again.'; got 'The user ARN arn:aws:iam::<account>:user/<name> is already registered with the on-premises instance <name>'.
6. codedeploy.register-onprem-instance-duplicate-iam-session-arn-rejected — expected IamSessionArnAlreadyRegisteredException + 'The on-premises instance could not be registered because the request included an IAM session ARN that has already been used to register a different instance. Include either a different IAM session ARN or IAM user ARN in the request, and then try again.'; got 'The IAM session ARN arn:aws:sts::<account>:assumed-role/<role>/<session> has already been used to register the on-premises instance <name>'.
7. codedeploy.put-lifecycle-event-hook-status-deployment-not-exist-rejected — expected message 'The lifecycleEventHookExecutionId is invalid'; got 'The lifecycleEventHookExecutionId <hookId> is invalid: it names no lifecycle event hook execution of deployment <deploymentId>'.
8. codedeploy.tag-resource-invalid-arn-rejected — expected ('InvalidArnException', 'Invalid ARN: not-a-valid-arn'); got ('InvalidArnException', 'The ARN not-a-valid-arn is not in a valid format').
9. codedeploy.stop-deployment-already-completed-blocked — expected errorInformation {'code':'NO_INSTANCES','message':'The deployment failed because no instances were found for your deployment group. Check your deployment group settings to make sure the tags for your Amazon EC2 instances or Auto Scaling groups correctly identify the instances you want to deploy to, and then try again.'}; got {'code':'NO_INSTANCES','message':'No instances found for the deployment group of this deployment'}.

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): CreateDeploymentConfig models InvalidMinimumHealthyHostValueException (400); DeleteDeploymentConfig models InvalidOperationException (400); CreateDeploymentGroup models InvalidOnPremisesTagCombinationException and InvalidEC2TagCombinationException (400); RegisterOnPremisesInstance models IamUserArnAlreadyRegisteredException and IamSessionArnAlreadyRegisteredException (400); PutLifecycleEventHookExecutionStatus models InvalidLifecycleEventHookExecutionIdException (400); TagResource models InvalidArnException (400). The model documents no message text (member doc is only the generic exception-message doc), so the texts above are AWS's recorded wording for those error classes.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite; the nine clusters reproduce.
2. Cause 1: CreateDeploymentConfig(deploymentConfigName=<new>, minimumHealthyHosts={'type':'FLEET_PERCENT','value':-5}) and compare the Message.
3. Cause 2: DeleteDeploymentConfig(deploymentConfigName='CodeDeployDefault.OneAtATime').
4. Cause 3: CreateApplication(Server) then CreateDeploymentGroup(..., onPremisesInstanceTagFilters=[f], onPremisesTagSet={'onPremisesTagSetList':[[f]]}).
5. Cause 4: CreateDeploymentGroup(..., ec2TagFilters=[f], ec2TagSet={'ec2TagSetList':[[f]]}).
6. Cause 5: RegisterOnPremisesInstance(instanceName=A, iamUserArn=X) then RegisterOnPremisesInstance(instanceName=B, iamUserArn=X).
7. Cause 6: RegisterOnPremisesInstance(instanceName=A, iamSessionArn=S) then RegisterOnPremisesInstance(instanceName=B, iamSessionArn=S).
8. Cause 7: create a real deployment, then PutLifecycleEventHookExecutionStatus(deploymentId=<that id>, lifecycleEventHookExecutionId='sim2real-hook-1', status='Succeeded').
9. Cause 8: TagResource(ResourceArn='not-a-valid-arn', Tags=[{'Key':'A','Value':'B'}]).
10. Cause 9: create a Server deployment group whose onPremisesInstanceTagFilters match nothing, CreateDeployment against it, poll GetDeployment until Failed and read errorInformation.message.
