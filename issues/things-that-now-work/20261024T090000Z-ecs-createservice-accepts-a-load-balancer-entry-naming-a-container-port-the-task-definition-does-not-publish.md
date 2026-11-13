Issue-Severity: LOW

# ECS CreateService accepts a load-balanced service whose task definition cannot carry a load balancer

> _Filed by the fixer of
> `20261023T060000Z-ecs-elbv2-createservice-accepts-an-instance-type-target-group-for-an-awsvpc-task-definition.md`.
> The merge-gate reviewer of that fix asked for the leniency it leaves behind to be
> recorded rather than implied._

## Operation
CreateService, UpdateService (`com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113`)

## Description
`CreateService` now validates each `loadBalancers` entry's target group against the task
definition's `networkMode`
(`services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/state.rs`,
`required_target_type` / `load_balancer_refusal`): `awsvpc` requires an `ip` target group
and every other mode an `instance` one. That rule is about the TARGET TYPE only. Two
further requirements a load-balanced ECS service has are not checked:

1. The container the entry names has to exist in the task definition and to publish the
   `containerPort` the entry names — AWS refuses an entry naming a container or a port the
   revision does not declare. The `none` network mode cannot declare a port mapping at all
   ("If the network mode is set to none, you cannot specify port mappings in your container
   definitions"), so a load-balanced service on a `none` revision has no network path for
   the load balancer and AWS refuses it, where the emulator accepts it with an `instance`
   target group.
2. `LoadBalancer$loadBalancerName` (a Classic Elastic Load Balancing attachment) is stored
   verbatim and checked against nothing: the load balancer it names lives in the Classic
   Elastic Load Balancing service's store, and the `awsvpc` network mode is not supported
   behind a Classic load balancer.

## Expected
`CreateService` and `UpdateService` refuse, with the modelled `InvalidParameterException`
(HTTP 400):

* a `loadBalancers` entry whose `containerName` is not a container of the resolved
  task-definition revision, or whose `containerPort` is not a port that container
  publishes — which makes a load-balanced service on a `none`-mode revision a refusal by
  itself;
* a `loadBalancerName` that names no Classic load balancer (a cross-service read of that
  service's store, as the target-group check reads the ELBv2 store), and one paired with an
  `awsvpc` revision.

## Notes
The target-type half of the pairing, the cross-service read it is made through and the
`loadBalancers` list bound are in place; see
`services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/tests/all/load_balancer_target_groups.rs`,
whose table deliberately uses the `none` network mode only in a refusal row for the target
type, because this issue's rule is the one that would refuse it outright.
