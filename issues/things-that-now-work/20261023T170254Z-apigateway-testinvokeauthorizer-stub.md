Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.authorizer-cache-ttl-window apigateway.test-invoke-authorizer-enforces-identity-source apigateway.test-invoke-authorizer-executes-configured-authorizer apigateway.test-invoke-authorizer-executes-lambda-policy apigateway.test-invoke-authorizer-missing-identity-source-rejected apigateway.test-invoke-authorizer-not-found apigateway.test-invoke-authorizer-returns-policy-and-claims
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|TestInvokeAuthorizer|NotImplemented
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# Implement apigateway TestInvokeAuthorizer: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeAuthorizer

## Description
`TestInvokeAuthorizer` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_authorizer.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `TestInvokeAuthorizer` call, so each one unblocks only when `TestInvokeAuthorizer` works: `apigateway.authorizer-cache-ttl-window`, `apigateway.test-invoke-authorizer-enforces-identity-source`, `apigateway.test-invoke-authorizer-executes-configured-authorizer`, `apigateway.test-invoke-authorizer-executes-lambda-policy`, `apigateway.test-invoke-authorizer-missing-identity-source-rejected`, `apigateway.test-invoke-authorizer-not-found`, `apigateway.test-invoke-authorizer-returns-policy-and-claims`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_authorizer.rs`. Replaying the corpus, the emulator refused the `TestInvokeAuthorizer` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TestInvokeAuthorizer operation: operation TestInvokeAuthorizer is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigateway emulator
Call TestInvokeAuthorizer and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
