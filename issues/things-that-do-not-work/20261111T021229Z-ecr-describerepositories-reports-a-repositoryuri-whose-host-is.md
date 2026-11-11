Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.ecracc-describe-unknown-repository-rejected
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.ecracc-describe-unknown-repository-rejected|run#9|AssertionError|assert host.startswith(f"{found[0]['registryId']}.dkr.ecr.{ctx.region}."), (
Issue-Commit: 1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30

# ECR DescribeRepositories reports a repositoryUri whose host is not the account's ECR registry DNS name (<registryId>.dkr.ecr.<region>.amazonaws.com)

> _Note: this issue was identified against the code base at commit `1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRepositories

## Description
Cause 1 of 1: the repositoryUri the emulator reports for an ordinary (non pull-through-cache) repository has no ECR registry host at all, so a client cannot derive the registry id, the account or the region from the URI it is told to push and pull with.

AWS: Smithy: ecr Repository.repositoryUri (target Url) — 'The URI for the repository. You can use this URI for container image push and pull operations.' AWS always forms it as '<registryId>.dkr.ecr.<region>.amazonaws.com/<repositoryName>', where registryId is the 12-digit account that owns the registry (Repository.registryId, pattern ^[0-9]{12}$) and region is the service region; the fixed parts are '.dkr.ecr.' and '.amazonaws.com', and the path is exactly Repository.repositoryName. Repository.repositoryArn is correspondingly 'arn:aws:ecr:<region>:<registryId>:repository/<repositoryName>'. The host is therefore derived from state (registry id, region), not from the environment.

Emulator: DescribeRepositories returns a repositoryUri whose host is a bare host:port with no registry DNS name — the replay shows '127.0.0.1:42591/sim2real-ecracc-repo-<id>' (the path is correct, the host is not). The particular address and port are environment-specific and are not the requirement; the requirement is the '<registryId>.dkr.ecr.<region>.amazonaws.com/<repositoryName>' shape.

Why: every client that parses repositoryUri breaks — it cannot recover the account or region to build ecr: resource ARNs or a 'docker login' target, and any task definition, Lambda ImageUri or buildspec derived from the reported URI references a registry that is not the account's ECR. The check aborts at this assertion, so the rest of the DescribeRepositories contract it covers (a name listed twice is not deduplicated; one absent name beside valid ones fails the whole call with RepositoryNotFoundException for that name) is never even reached.

Fix: the ECR repository representation/serializer — the component that computes repositoryUri for CreateRepository and DescribeRepositories must emit the registry-DNS form from the repository's registryId and the service region. Related open issues: 20261111T010150Z-ecr-ecs-logs-ecr-pull-through-cache-repository-uri-is-not.md records the same shape defect only for pull-through-cache repositories under ecr__ecs__logs (it does not carry this check), and 20261109T141609Z-codebuild-ecr-ecs-elbv2-logs-the-ecr-repository.md asks that the announced URI also be reachable — a fix must satisfy both: announce the registry-DNS form and make that host resolvable/served locally rather than publishing a loopback address in the API body.

Traces: ecr.ecracc-describe-unknown-repository-rejected

## Evidence
Expected (AWS): DescribeRepositories(repositoryNames=[<name>]) -> repositories[0].repositoryUri == '<registryId>.dkr.ecr.<region>.amazonaws.com/<name>', i.e. the host starts with '<repositories[0].registryId>.dkr.ecr.<region>.'.
Actual (emulator): AssertionError: the URI host must start with registry id and region; '127.0.0.1:42591/sim2real-ecracc-repo-b6a7a507894c' — the path equals the repository name, but the host carries no registry id, no '.dkr.ecr.' and no region.

## Steps to Reproduce
1. ecr CreateRepository with a valid repositoryName.
2. ecr DescribeRepositories(repositoryNames=[<that name>]) and read repositories[0].repositoryUri and repositories[0].registryId.
3. Split the URI on the first '/': AWS host == '<registryId>.dkr.ecr.<region>.amazonaws.com'; emulator host == '127.0.0.1:<port>'.
4. Replay ecr.ecracc-describe-unknown-repository-rejected.
