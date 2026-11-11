Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.delete-namespace-blocked-by-nonempty-precondition quicksight.theme-creation-reaches-successful-state
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight.delete-namespace-blocked-by-nonempty-precondition|run#20|AssertionError|assert dead_err["Message"] == (
Issue-Key: quicksight.theme-creation-reaches-successful-state|run#18|AssertionError|assert not_found_err["Message"] == theme_gone, not_found_err
Issue-Commit: 5f88a7dcadd1e69adbda000c9f17c3a3daf7e228

# QuickSight ResourceNotFoundException messages for namespaces and themes are emulator-authored instead of AWS's templates

> _Note: this issue was identified against the code base at commit `5f88a7dcadd1e69adbda000c9f17c3a3daf7e228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteNamespace

## Description
Two net-new root causes in the QuickSight emulator, both the same fault class but on two distinct resource paths (namespace, theme): the emulator raises the right error shape (ResourceNotFoundException) and the right HTTP status (404), but writes its own message sentence, so the text a client reads is wording AWS never emits. For these error classes the message is part of the contract. This bundle has 2 causes; BOTH must be closed before submitting.

Related already-open issues describe the same wording fault on OTHER resource paths and do NOT cover these two: 20261031T224711Z-quicksight-data-source-resourcenotfoundexception-message-is-invented-wording.md (data source), 20261103T040231Z-quicksight-data-set-resourcenotfoundexception-message-is-invented-wording.md (data set), and 20261109T121538Z-quicksight-not-found-and-already-exists-error-messages.md (CreateGroup / CreateFolder / DescribeDataSource). Neither the namespace nor the theme not-found message appears in any of them, so they are filed here with their own behavioral check ids.

Cause 1 of 2: DeleteNamespace on an already-deleted namespace.
AWS: DeleteNamespace (Smithy: DELETE /accounts/{AwsAccountId}/namespaces/{Namespace}; modeled errors include ResourceNotFoundException, errorKind client, httpError 404). Re-deleting a namespace that is already gone answers HTTP 404, Code ResourceNotFoundException, with the namespace-delete template: fixed text "Namespace ", " not found Failed to delete namespace ", " for account ", ". " — note it names the namespace twice and ends with a period followed by a trailing space. The variable parts are the request's Namespace path label (twice) and its AwsAccountId path label. The model fixes the shape and the status but not the text, so the template itself is recorded behavior only.
Emulator: HTTP 404, Code ResourceNotFoundException (both correct) with message "Namespace <Namespace> not found in account <AwsAccountId>." — an emulator-authored sentence using the template "Namespace {Namespace} not found in account {AwsAccountId}."
Why: a client that surfaces, logs or pattern-matches QuickSight's namespace-delete failure text — or parses the "Failed to delete namespace ... for account ..." clause out of it to distinguish a delete-path failure from a plain lookup miss — sees wording that never appears on AWS and finds none of the fragments it expects.
Fix: the not-found error construction on DeleteNamespace's handler (services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/delete_namespace.rs) — render AWS's template from the request's Namespace and AwsAccountId instead of an emulator-authored sentence.
Traces: quicksight.delete-namespace-blocked-by-nonempty-precondition.

Cause 2 of 2: DescribeTheme (and CreateThemeAlias) on a deleted theme.
AWS: DescribeTheme (Smithy: GET /accounts/{AwsAccountId}/themes/{ThemeId}; modeled errors include ResourceNotFoundException, client, httpError 404) and CreateThemeAlias (POST /accounts/{AwsAccountId}/themes/{ThemeId}/aliases/{AliasName}; same modeled 404). After DeleteTheme, both answer HTTP 404 / ResourceNotFoundException with the ARN-based theme template: fixed text "Theme " and " is not found" (no trailing period) around one variable part, the theme ARN arn:aws:quicksight:<region>:<AwsAccountId>:theme/<ThemeId>, assembled from the serving region and the request's path labels. Recorded behavior only for the text; the model fixes shape and status.
Emulator: HTTP 404, Code ResourceNotFoundException (both correct) with message "Theme with ID <ThemeId> not found." — template "Theme with ID {ThemeId} not found.", no ARN, trailing period. The behavioral check fails on DescribeTheme before it reaches the CreateThemeAlias assertion, which asserts the identical message; expect the same wording defect there, so the fix must cover both paths.
Why: the emulator's message carries no ARN, so anything that extracts the missing theme's ARN from the error (to report which theme of a batch is gone, or to resolve its account/region) finds nothing, and message-sensitive assertions diverge.
Fix: the not-found error construction shared by the theme handlers (ops/describe_theme.rs and ops/create_theme_alias.rs) — build the theme ARN using the same derivation already used for a created theme's Arn and render AWS's template.
Traces: quicksight.theme-creation-reaches-successful-state.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against the emulator (HTTP status and error Code matched on both; only the message text diverges).

1) quicksight.delete-namespace-blocked-by-nonempty-precondition — CreateNamespace, poll to CREATED, DeleteNamespace, poll until unreachable, then DeleteNamespace again on the dead id.
   Expected (AWS): Error.Code = ResourceNotFoundException, Error.Message = "Namespace <Namespace> not found Failed to delete namespace <Namespace> for account <AwsAccountId>. " — fixed text "Namespace " / " not found Failed to delete namespace " / " for account " / ". " (trailing space); variable parts = the request's Namespace (twice) and AwsAccountId.
   Actual (emulator): {'Code': 'ResourceNotFoundException', 'Message': 'Namespace <Namespace> not found in account <AwsAccountId>.'} (observed with the generated namespace name and the emulator's 12-digit account id filling the template).
   Check assertion: `assert dead_err["Message"] == f"Namespace {namespace} not found Failed to delete namespace {namespace} for account {account_id}. "` -> AssertionError.

2) quicksight.theme-creation-reaches-successful-state — CreateTheme(BaseThemeId=MIDNIGHT, DataColorPalette), poll to CREATION_SUCCESSFUL, DeleteTheme, then DescribeTheme on the deleted id.
   Expected (AWS): Error.Code = ResourceNotFoundException, Error.Message = "Theme arn:aws:quicksight:<region>:<AwsAccountId>:theme/<ThemeId> is not found" — fixed text "Theme " / " is not found"; variable part = the theme ARN built from the serving region and the request's AwsAccountId/ThemeId. The same message is expected from CreateThemeAlias on the deleted theme.
   Actual (emulator): {'Code': 'ResourceNotFoundException', 'Message': 'Theme with ID <ThemeId> not found.'}
   Check assertion: `assert not_found_err["Message"] == theme_gone` -> AssertionError (the run never reaches the CreateThemeAlias assertion).

Smithy citations (services/service-com-amazonaws-quicksight-quicksight-20180401/smithy-model/com-amazonaws-quicksight-quicksight-20180401.json): DeleteNamespace errors -> ResourceNotFoundException (client, httpError 404); DescribeTheme errors -> ResourceNotFoundException (client, httpError 404); CreateThemeAlias errors -> ResourceNotFoundException (client, httpError 404). None of them fixes the message text, so the two templates above are recorded behavior only — the names, ids and account in any single run only fill them.

## Steps to Reproduce
Cause 1 (DeleteNamespace):
1. Subscribe the account (CreateAccountSubscription ENTERPRISE / IAM_ONLY) if it is not already ACCOUNT_CREATED.
2. CreateNamespace(AwsAccountId, Namespace=<fresh name>, IdentityStore=QUICKSIGHT) and poll DescribeNamespace until CreationStatus == CREATED.
3. DeleteNamespace(AwsAccountId, Namespace) and poll DescribeNamespace until it answers ResourceNotFoundException.
4. Call DeleteNamespace(AwsAccountId, Namespace) again and read Error.Message: the emulator answers "Namespace <ns> not found in account <account>." where AWS answers "Namespace <ns> not found Failed to delete namespace <ns> for account <account>. ".

Cause 2 (DescribeTheme / CreateThemeAlias):
1. With the account subscribed, CreateTheme(AwsAccountId, ThemeId=<fresh id>, Name, BaseThemeId=MIDNIGHT, Configuration={DataColorPalette:{Colors:[...]}}) and poll DescribeTheme until Theme.Version.Status == CREATION_SUCCESSFUL.
2. DeleteTheme(AwsAccountId, ThemeId).
3. Call DescribeTheme(AwsAccountId, ThemeId) and read Error.Message: the emulator answers "Theme with ID <id> not found." where AWS answers "Theme arn:aws:quicksight:<region>:<account>:theme/<id> is not found".
4. Call CreateThemeAlias(AwsAccountId, ThemeId, AliasName=POSTDELETE, ThemeVersionNumber=1) and read Error.Message: the same ARN-based text is expected there.
