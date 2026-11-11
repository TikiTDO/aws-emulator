Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull
Issue-Key: ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task|_pull_through_cache_repo#4|AssertionError|assert ".dkr.ecr." in uri, uri
Issue-Key: ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull|_pull_through_cache_repo#3|AssertionError|assert ".dkr.ecr." in uri, uri
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# ECR pull-through-cache repository URI is not an ECR registry URI: repositoryUri lacks the '.dkr.ecr.<region>.amazonaws.com' registry host

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePullThroughCacheRule

## Description
Cause 1 of 1: the repositoryUri the emulator reports for a pull-through-cache repository is not in ECR's registry-URI form, so every client that derives the registry (or the account and region) from it fails before it can even run a task.

AWS: Smithy: ECR Repository.repositoryUri — "The URI for the repository. You can use this URI for container image push and pull operations." AWS always forms it as `<registryId>.dkr.ecr.<region>.amazonaws.com/<repositoryName>` (for a pull-through cache, `<registryId>.dkr.ecr.<region>.amazonaws.com/<ecrRepositoryPrefix>/<upstream path>`), i.e. the host is the account's ECR registry DNS name and the path is the repository name. The variable parts are the registry id, the region and the repository name, taken from the request/account; the fixed part is the `.dkr.ecr.` + `.amazonaws.com` registry host form.

Emulator: repositoryUri comes back as a bare host:port with no ECR registry host at all — e.g. `127.0.0.1:36347/sim2real-xsvc-ptc-8071ea539e03/docker/library/busybox:latest` and `127.0.0.1:42935/sim2real-xsvc-ptc-345d0c69e6fd/docker/library/busybox:latest` ('AssertionError: ...' on `".dkr.ecr." in uri`). The specific host and port are environment-specific and are not the requirement; the requirement is the `<registryId>.dkr.ecr.<region>.amazonaws.com/<name>` shape.

Why: a client cannot parse the registry id or region out of the URI, cannot build the `ecr:` resource ARNs or docker login target from it, and any ECS task definition built from the reported URI references a registry that is not the account's ECR. Both replayed pull-through-cache checks abort at the very first step, so none of the downstream behaviour (cache population, execution-role authorization failures) is even reachable.

Fix: the ECR service's repository representation/serializer — the component that computes repositoryUri for CreateRepository, DescribeRepositories and the pull-through-cache repository created on first pull must emit the registry-DNS form from the request's registry id and the service region.

Traces: ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task, ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull (both fail on the same assertion in _pull_through_cache_repo).

## Evidence
Expected (AWS): repositoryUri matches `<registryId>.dkr.ecr.<region>.amazonaws.com/<ecrRepositoryPrefix>/<upstream-path>` — it contains '.dkr.ecr.'.
Actual (emulator): '127.0.0.1:36347/sim2real-xsvc-ptc-8071ea539e03/docker/library/busybox:latest' (and '127.0.0.1:42935/sim2real-xsvc-ptc-345d0c69e6fd/...'): no registry host, no account id, no region. AssertionError on `'.dkr.ecr.' in uri`.

## Steps to Reproduce
1. CreatePullThroughCacheRule with an ecrRepositoryPrefix and upstreamRegistry=public ECR / Docker Hub.
2. Trigger the cache repository's creation (pull or CreateRepository for '<prefix>/docker/library/busybox') and read repositoryUri from the CreateRepository/DescribeRepositories response.
3. AWS: '<acct>.dkr.ecr.<region>.amazonaws.com/<prefix>/docker/library/busybox'. Emulator: '127.0.0.1:<port>/<prefix>/docker/library/busybox:latest'.
