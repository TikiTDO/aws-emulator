Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-create-workspaces-batch-start-stop-reboot
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-create-workspaces-batch-start-stop-reboot
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorCode
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorMessage
Issue-Commit: f7fb1918db1122816ff8c73ca397fdda050b2857

# CreateWorkspaces reports a batch element's per-item failure against the wrong resource: ResourceNotFound.Directory where AWS answers ResourceNotFound.Bundle

> _Note: this issue was identified against the code base at commit `f7fb1918db1122816ff8c73ca397fdda050b2857`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkspaces

## Description
One cause.

Cause 1 of 1: CreateWorkspaces misattributes a per-element failure in FailedRequests — it reports the DIRECTORY as the unresolvable resource for an element AWS reports the BUNDLE for (fixture workspaces-create-workspaces-batch-start-stop-reboot, step task#3, FailedRequests[1]).

AWS: CreateWorkspaces is a batch operation that judges each WorkspaceRequest on its own and returns HTTP 200 whatever happens to the individual elements (Smithy: CreateWorkspaces -> CreateWorkspacesResult{FailedRequests: FailedCreateWorkspaceRequests, PendingRequests: WorkspaceList}; the operation models only InvalidParameterValuesException and ResourceLimitExceededException, neither of which covers an unresolvable per-element resource). A refused element is reported as a FailedCreateWorkspaceRequest whose ErrorCode (target ErrorType, "The error code that is returned if the WorkSpace cannot be created" — a free-form per-item code, no enum) and ErrorMessage (target Description) name THE RESOURCE THAT ACTUALLY FAILED TO RESOLVE. WorkspaceRequest requires both DirectoryId (pattern ^(d-[0-9a-f]{8,63}$)|(wsd-[0-9a-z]{8,63}$)$) and BundleId (pattern ^wsb-[0-9a-z]{8,63}$). The model publishes no value list for ErrorType, so the codes and their wording are recorded behavior only: for the element in question the recording carries ErrorCode 'ResourceNotFound.Bundle' and ErrorMessage 'The specified bundle could not be found in the specified region.' In that message the whole sentence is FIXED text for this error class — it interpolates nothing from the request or from state (it does not name the bundle id or the region), and the same is true of the directory-flavoured sibling 'The specified directory could not be found in the specified region.' The requirement is therefore which of the two fixed messages is emitted, paired with its matching code: a bundle that does not resolve must yield ResourceNotFound.Bundle plus the bundle sentence.

Emulator: for the same element the emulator answers HTTP 200 (status matches) but fills that FailedRequests entry with ErrorCode 'ResourceNotFound.Directory' and ErrorMessage 'The specified directory could not be found in the specified region.' — the directory-flavoured pair — where the recording has the bundle-flavoured pair. Replay reports exactly: $.FailedRequests[1].ErrorCode value_mismatch (expected 'ResourceNotFound.Bundle', got 'ResourceNotFound.Directory') and $.FailedRequests[1].ErrorMessage value_mismatch (expected 'The specified bundle could not be found in the specified region.', got 'The specified directory could not be found in the specified region.'). Every other element of the same batch matched, so the emulator's directory registry and bundle catalog are not broken wholesale — only this element's attribution is.

Hypothesis: the element is unresolvable on BOTH members, and the two implementations break the tie differently. The emulator's per-element judgement tests the directory first and returns on the first failure, and its own documentation states the tiebreak it chose as a choice rather than an observation ("An element whose directory AND bundle are both unresolvable has two failures to report in one entry; it is reported against the FIRST of them in the model's own member order (WorkspaceRequest declares DirectoryId before BundleId)"). The recording shows AWS resolving that same element against the bundle, so AWS does not use model member order as the precedence. The alternative reading — that this element's DirectoryId does resolve in AWS and the emulator wrongly judged it unregistered — produces the identical symptom; either way the emitted code/message pair names the wrong resource.

Why: ErrorCode is the only thing a client can branch on for a per-element batch failure, because the call itself succeeded with 200. A client that retries a failed element after fixing what the code named will "fix" the directory (re-register it, or correct DirectoryId) and resubmit an element that still carries an unresolvable BundleId, so the retry fails identically and the loop never terminates; operator-facing output and alarm text likewise blame directory registration for a bundle-catalog problem. Any logic keyed on the ResourceNotFound.Bundle code never fires against the emulator.

Fix: the per-element judgement in the CreateWorkspaces state transition — WorkspacesState::create_workspace, the directory-then-bundle early-return chain that selects (ERROR_CODE_DIRECTORY_NOT_FOUND, MESSAGE_DIRECTORY_NOT_FOUND) versus (ERROR_CODE_BUNDLE_NOT_FOUND, MESSAGE_BUNDLE_NOT_FOUND) before returning its ItemFailure to the CreateWorkspaces handler. The handler itself only echoes whatever pair it is handed, so the precedence between the two unresolved-resource conditions (and, if the directory in fact resolves, the directory lookup that says otherwise) is what has to change; the bundle condition must win for an element whose bundle does not resolve.

Traces: workspaces-create-workspaces-batch-start-stop-reboot#task#3 (CreateWorkspaces, expected 200, actual 200, two value mismatches under $.FailedRequests[1]).

Related open issues, neither of which covers this: 20261105T054826Z-workspaces-resourcenotfoundexception-for-a-missing-ip-group-carries-the (top-level ResourceNotFoundException wording for IP groups in the tagging operations) and 20261108T072651Z-workspaces-describeworkspacebundles-nexttoken-is-an-internal-positional-offset-not (DescribeWorkspaceBundles pagination tokens). This cause is a per-item ErrorType attribution inside a 200 CreateWorkspaces response.

## Evidence
Fixture workspaces-create-workspaces-batch-start-stop-reboot, step task#3, operation CreateWorkspaces, expected HTTP 200 / actual HTTP 200.

Expected (recorded AWS), FailedRequests[1]:
  ErrorCode:    "ResourceNotFound.Bundle"
  ErrorMessage: "The specified bundle could not be found in the specified region."

Actual (emulator), FailedRequests[1]:
  ErrorCode:    "ResourceNotFound.Directory"
  ErrorMessage: "The specified directory could not be found in the specified region."

Replay detail verbatim:
  $.FailedRequests[1].ErrorCode: value_mismatch (expected 'ResourceNotFound.Bundle', got 'ResourceNotFound.Directory')
  $.FailedRequests[1].ErrorMessage: value_mismatch (expected 'The specified bundle could not be found in the specified region.', got 'The specified directory could not be found in the specified region.')

Message anatomy: both candidate messages are wholly fixed text for their error class — no request value, resource id, region or timestamp is interpolated — so the divergence is purely which (code, message) pair the element is judged into. FailedRequests[0] and the batch's remaining elements matched, as did the 200 status and PendingRequests.

Model evidence: Smithy FailedCreateWorkspaceRequest{WorkspaceRequest, ErrorCode: ErrorType, ErrorMessage: Description}; CreateWorkspaces errors = InvalidParameterValuesException(400), ResourceLimitExceededException(400) only; WorkspaceRequest requires DirectoryId and BundleId. ErrorType is an unconstrained string, so the two code spellings and their messages are recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture workspaces-create-workspaces-batch-start-stop-reboot up to step task#3, which issues CreateWorkspaces with a batch whose second element cannot be created (its BundleId names no bundle in the catalog).
3. Read the 200 response body's FailedRequests[1].
4. Observe ErrorCode 'ResourceNotFound.Directory' and ErrorMessage 'The specified directory could not be found in the specified region.' where the recording has 'ResourceNotFound.Bundle' and 'The specified bundle could not be found in the specified region.'; FailedRequests[0] and the rest of the response match.
