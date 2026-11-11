Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.nodejs-function-sdk-creates-lists-and-deletes-a-bucket lambda__s3.nodejs-function-sdk-delete-objects lambda__s3.nodejs-function-sdk-multipart-upload-round-trip lambda__s3.nodejs-function-sdk-object-tagging-round-trips lambda__s3.nodejs-function-sdk-paginate-list-objects-v2 lambda__s3.nodejs-function-sdk-s3-presigner-url-fetches-the-object lambda__s3.nodejs-handler-https-get-fetches-a-presigned-url lambda__s3.nodejs-function-sdk-last-modified-is-a-date
Issue-Key: lambda__s3.nodejs-function-sdk-creates-lists-and-deletes-a-bucket|_invoke_granted#7|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.nodejs-function-sdk-delete-objects|_invoke_granted#9|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.nodejs-function-sdk-multipart-upload-round-trip|_invoke_granted#13|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.nodejs-function-sdk-object-tagging-round-trips|_invoke_granted#8|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.nodejs-function-sdk-paginate-list-objects-v2|_invoke_granted#8|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.nodejs-function-sdk-s3-presigner-url-fetches-the-object|_invoke_granted#8|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.nodejs-handler-https-get-fetches-a-presigned-url|_invoke_ok#8|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.nodejs-function-sdk-last-modified-is-a-date|run#8|AssertionError|assert got["isDate"] is True, f"{where} LastModified is a {got['type']!r}, not a Date"
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# The Lambda Node.js runtime's bundled SDK and module resolution diverge: missing S3 operations, paginator, presigner, node builtins and Date timestamps

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Five net-new root causes in the Node.js Lambda runtime the emulator serves to functions. Each is a property of the runtime inside the function, not of the S3 service.

Cause 1 of 5: @aws-sdk/client-s3 in the runtime binds only six S3 operations; every other command raises instead of being sent.
AWS: recorded behavior only for the runtime contents — a Node.js Lambda's bundled @aws-sdk/client-s3 binds the full S3 API, so ListBucketsCommand, DeleteObjectsCommand, CreateMultipartUploadCommand/UploadPartCommand/CompleteMultipartUploadCommand and PutObjectTaggingCommand all reach S3.
Emulator: the handler faults with errorType 'UnrecognizedClientException' and message 'the AWS SDK this runtime supplies binds no <Operation> call on Amazon S3; the operations it binds are GetObject, HeadObject, PutObject, CopyObject, DeleteObject, ListObjectsV2', thrown from /var/runtime/node_modules/@aws-sdk/client-s3.
Why: any Node handler beyond single-object I/O cannot run, and the failure is reported as a client/credentials exception, which sends a caller debugging in the wrong direction.
Fix: the Node.js runtime's bundled @aws-sdk/client-s3 — bind the S3 operations the service implements instead of a six-operation allowlist.
Traces: nodejs-function-sdk-creates-lists-and-deletes-a-bucket, nodejs-function-sdk-delete-objects, nodejs-function-sdk-multipart-upload-round-trip, nodejs-function-sdk-object-tagging-round-trips.

Cause 2 of 5: the bundled @aws-sdk/client-s3 exports no paginator.
AWS: the module exports paginateListObjectsV2, and importing it in a handler works.
Emulator: errorType 'Runtime.UserCodeSyntaxError', message "SyntaxError: Could not find export 'paginateListObjectsV2' in module 'file:///var/runtime/node_modules/@aws-sdk/client-s3'" — the function fails at load.
Why: the documented pagination helper cannot be imported, so the handler never initialises.
Fix: the bundled @aws-sdk/client-s3's exports — provide the paginate* helpers.
Traces: nodejs-function-sdk-paginate-list-objects-v2.

Cause 3 of 5: the runtime ships no @aws-sdk/s3-request-presigner.
AWS: a Node handler can import getSignedUrl from @aws-sdk/s3-request-presigner and presign a GetObject.
Emulator: errorType 'Runtime.ImportModuleError', message "Error: Cannot find module '@aws-sdk/s3-request-presigner'".
Why: presigning from inside a function is impossible.
Fix: the Node.js runtime's bundled module set.
Traces: nodejs-function-sdk-s3-presigner-url-fetches-the-object.

Cause 4 of 5: Node built-in modules are not resolvable under the node: specifier.
AWS: import http from 'node:http' (and node:https) resolves in every supported Node runtime.
Emulator: errorType 'Runtime.ImportModuleError', message "Error: Cannot find module 'node:http'".
Why: ordinary Node code that uses the standard prefixed builtins fails at import, so the handler never runs.
Fix: the Node.js runtime's module resolver — resolve the node: prefixed builtins.
Traces: nodejs-handler-https-get-fetches-a-presigned-url.

Cause 5 of 5: timestamps are numbers, not Date objects.
AWS: the JS SDK deserialises HeadObject's LastModified into a Date.
Emulator: 'head LastModified is a 'number', not a Date'.
Why: handler code calling .toISOString() or doing date arithmetic on LastModified throws.
Fix: the bundled SDK's response deserialisation for timestamp members.
Traces: nodejs-function-sdk-last-modified-is-a-date.

## Evidence
All under lambda__s3 [mismatch]:
- nodejs-function-sdk-creates-lists-and-deletes-a-bucket / -delete-objects / -multipart-upload-round-trip / -object-tagging-round-trips: errorType 'UnrecognizedClientException', errorMessage 'the AWS SDK this runtime supplies binds no ListBuckets|DeleteObjects|CreateMultipartUpload|PutObjectTagging call on Amazon S3; the operations it binds are GetObject, HeadObject, PutObject, CopyObject, DeleteObject, ListObjectsV2', trace at notMade (/var/runtime/node_modules/@aws-sdk/client-s3:196:11)
- nodejs-function-sdk-paginate-list-objects-v2: 'Runtime.UserCodeSyntaxError' — "SyntaxError: Could not find export 'paginateListObjectsV2' in module 'file:///var/runtime/node_modules/@aws-sdk/client-s3'"
- nodejs-function-sdk-s3-presigner-url-fetches-the-object: 'Runtime.ImportModuleError' — "Error: Cannot find module '@aws-sdk/s3-request-presigner'"
- nodejs-handler-https-get-fetches-a-presigned-url: 'Runtime.ImportModuleError' — "Error: Cannot find module 'node:http'"
- nodejs-function-sdk-last-modified-is-a-date: 'head LastModified is a 'number', not a Date'

## Steps to Reproduce
1. CreateFunction with a Node.js runtime and an index.mjs handler importing @aws-sdk/client-s3.
2. Send any command outside {GetObject, HeadObject, PutObject, CopyObject, DeleteObject, ListObjectsV2} — UnrecognizedClientException with the 'binds no ...' message.
3. import { paginateListObjectsV2 } from '@aws-sdk/client-s3' — the function fails to load with a missing-export SyntaxError.
4. import { getSignedUrl } from '@aws-sdk/s3-request-presigner' — Cannot find module.
5. import http from 'node:http' — Cannot find module 'node:http'.
6. Return typeof/constructor of HeadObject's LastModified — 'number', not Date.
