// SPDX-License-Identifier: CC0-1.0

//! The success signals the INSTANCES of a signalled resource send, which is what releases the
//! resource-signal wait of a resource whose signals come from the instances it consists of.
//!
//! ## The rule this module carries
//!
//! A `CreationPolicy`'s `ResourceSignal` is released by the signals the resource's own
//! instances send: "you can use the `cfn-signal` helper script … to signal" and
//! `SignalResource` "is useful in cases where you want to send signals from anywhere other
//! than an Amazon EC2 instance", so for the two types AWS documents that bootstrap on — an
//! Auto Scaling group, whose launched instances each signal, and an instance, which signals
//! for itself — the signals are sent by the Amazon EC2 instances the resource consists of,
//! each naming its own instance id as the signal's `UniqueId` ("when you signal Amazon EC2
//! instances or Auto Scaling groups, specify the instance ID that you are signaling as the
//! unique ID").
//!
//! So a stack whose group carries a `CreationPolicy` and whose instances are told to signal
//! it reaches its terminal status once those instances are up, rather than waiting out a
//! `Timeout` nothing was ever going to signal inside: the signals are recorded for the
//! instances the OWNING service reports serving, under the ids that service assigned them,
//! and the wait those signals satisfy is released exactly as a client's own
//! `SignalResource` call releases one ([`crate::ops::signal_resource`]).
//!
//! Three things make such a signal due, and all three are read rather than assumed:
//!
//! * the resource's declaration asks for the wait (its `CreationPolicy`'s `ResourceSignal`,
//!   which is what opened the barrier — [`crate::state::wait_conditions`]);
//! * the service that owns the resource reports the instance SERVING — in service in the
//!   group, or running for an instance of its own — so nothing is signalled for an instance
//!   that is not up; and
//! * that instance's own BOOT CONFIGURATION, as Amazon EC2 holds it, asks it to signal THIS
//!   resource of its stack, and asks for a SUCCESS ([`asked_signal_status`]) — which is what
//!   decides, on AWS too, whether an instance sends a signal at all and which status it
//!   sends. A resource whose instances are not told to signal, or whose instances are told to
//!   report the failure of their own bootstrap, is held to its `Timeout` and fails there,
//!   exactly as before.
//!
//! What this records was owed at the instant the owning service reported the instance
//! serving, whether or not anybody called: that instant is when the instance signalled. It
//! is driven from [`super::advance_stabilisations_in`] for the reason every other call there
//! is — the answers come from other services, which nothing under this store's lock can
//! reach.
//!
//! ## Bounds
//!
//! One pass asks the owning service for one open gate's instances once and reads the boot
//! configuration of each instance of that gate that has not signalled yet at most once,
//! stopping as soon as it has found the signals the gate still needs
//! ([`InstanceGate::needed`]) — so the calls are bounded by the instances the owning service
//! lets one group hold, and only while such a gate is open and unsatisfied, which is nothing
//! at all for a stack that declares no `CreationPolicy`. Reading one configuration is bounded
//! too: at most a configuration's documented size is looked at
//! ([`MAX_BOOT_CONFIGURATION_BYTES`]) and the walk over it passes each byte once
//! ([`signal_invocations`]). The signals themselves land in the stack's own bounded log
//! ([`crate::state::MAX_SIGNALS_PER_STACK`]).

use super::*;

/// The resource types whose `CreationPolicy` signals are sent by the Amazon EC2 instances the
/// resource consists of, and how the instances of one are read.
///
/// These are the types AWS documents the signalling bootstrap on: an Auto Scaling group,
/// whose instances each signal the group's logical id, and an `AWS::EC2::Instance`, which
/// signals for itself. Every other type's signals come from a client's own `SignalResource`
/// call, and nothing here is asked about them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signalling {
    /// An Auto Scaling group: the instances are the ones Amazon EC2 Auto Scaling reports in
    /// service in it.
    AutoScalingGroup,
    /// An `AWS::EC2::Instance`: the instance is the resource itself, once Amazon EC2 reports
    /// it running.
    Instance,
}

impl Signalling {
    /// Which kind of resource `resource_type` is, or `None` for a type whose signals do not
    /// come from instances of its own.
    fn of(resource_type: &str) -> Option<Self> {
        if resource_type == crate::state::template::AUTOSCALING_GROUP {
            return Some(Self::AutoScalingGroup);
        }
        if resource_type == ec2_instance::EC2_INSTANCE {
            return Some(Self::Instance);
        }
        None
    }
}

/// One resource-signal wait a stack's create is open at whose signals come from the
/// resource's own instances: what a signal names it by, and the resource the instances belong
/// to.
#[derive(Debug, Clone)]
struct InstanceGate {
    /// The logical id the signals name — the `LogicalResourceId` of a `SignalResource` call.
    logical_id: String,
    /// The resource's type, which is what its events report it as.
    resource_type: String,
    /// The resource the instances belong to, as the owning service addresses it.
    physical_resource_id: String,
    /// How the instances of that resource are read.
    signalling: Signalling,
    /// How many MORE success signals this wait needs: its `Count`, less the signals the stack
    /// has already recorded for the resource. Nothing beyond that number can release the wait,
    /// so it is what a pass stops collecting at.
    needed: usize,
    /// The instances whose signal this stack has ALREADY recorded for the resource, by the
    /// unique id each was recorded under — which is that instance's own id.
    ///
    /// They are not read again: the signal they sent is already counted, so a pass looks at
    /// the instances that have not signalled yet, and a gate whose instances come into service
    /// over several passes still reaches its `Count`.
    signalled_already: BTreeSet<String>,
}

/// Record the success signals the instances of this Region's signalled resources have sent,
/// and release the waits those signals satisfy.
///
/// Called by [`super::advance_stabilisations_in`] before the waits that have RUN OUT are
/// failed, so a gate its instances have signalled is released rather than failed in the same
/// pass.
pub(super) async fn settle_instance_signals(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    state: &::engine::ServiceState<crate::state::ComAmazonawsCloudformationCloudformationState>,
    region: &str,
) {
    // Which gates are open, read under a lock of its own: the reads below reach other
    // services' handlers, which take those services' locks, so a client reading THIS service
    // must not be blocked behind them.
    let open: Vec<(String, Vec<InstanceGate>)> = {
        let store = state.lock();
        store
            .stacks
            .iter()
            .filter(|stack| stack.region == region)
            .filter_map(|stack| {
                let gates = open_instance_gates(stack);
                (!gates.is_empty()).then(|| (stack.stack_id.clone(), gates))
            })
            .collect()
    };
    for (stack_id, gates) in open {
        for gate in gates {
            // The instances the resource consists of, as the service that OWNS it reports
            // them now. One unanswered read is no evidence about them — a service is allowed
            // to throttle or be briefly unreachable — so the wait simply holds and the next
            // pass asks again.
            //
            // BOXED, like every other call this module's walks make from inside a future a
            // deep chain holds: each of these reads carries another service's whole request
            // chain, and holding one of them inline would put its state in THIS future — which
            // the caller holds on the stack while the rest of the pass below it runs
            // ([`super::advance_stabilisations_in`]).
            let Ok(instances) = Box::pin(serving_instances(ctx, region, &gate)).await else {
                continue;
            };
            // ...and, per instance, whether its own boot configuration asks it to signal the
            // stack, and with WHICH status. An instance Amazon EC2 holds no boot configuration
            // for was told nothing, so it signals nothing.
            //
            // What the pass stops at is the number of signals it has FOUND, not the number of
            // instances it has looked at: a group may hold instances told different things,
            // and the ones that signal are not necessarily the first the owning service
            // reports. The walk is bounded either way — it reads each serving instance of one
            // gate at most once, and that set is bounded by the capacity the owning service
            // lets a group have.
            let mut signalled: Vec<String> = Vec::new();
            let unsignalled = instances
                .into_iter()
                .filter(|instance_id| !gate.signalled_already.contains(instance_id));
            for instance_id in unsignalled {
                if signalled.len() >= gate.needed {
                    break;
                }
                let read = Box::pin(ec2_instance::boot_configuration(ctx, region, &instance_id));
                if let Ok(Some(boot)) = read.await {
                    // A configuration that asks for the FAILURE of its own bootstrap is
                    // asking this create to fail: no success signal is recorded for it, so the
                    // wait runs out and the create fails and rolls back
                    // ([`super::settle_expired_waits`]) — the terminal status such a stack
                    // reaches, reported on the `Timeout` rather than on the signal, since what
                    // failed that bootstrap is work this emulator did not run.
                    if asked_signal_status(&boot, &gate.logical_id)
                        == Some(crate::state::resource_signal_status::SUCCESS)
                    {
                        signalled.push(instance_id);
                    }
                }
            }
            if signalled.is_empty() {
                continue;
            }
            let now = SystemTime::now();
            let mut store = state.lock();
            let Some(stack) = store.resolve_mut(region, &stack_id) else {
                continue;
            };
            for instance_id in &signalled {
                // Keyed by the signalled resource and the signal's unique id, so an instance
                // already counted is not counted twice however many passes read it serving
                // ([`crate::state::StoredStack::record_signal`]).
                let first_time = stack.record_signal(
                    &gate.logical_id,
                    instance_id,
                    crate::state::resource_signal_status::SUCCESS,
                    now,
                );
                if !first_time {
                    continue;
                }
                let properties = stack
                    .current_resources()
                    .into_iter()
                    .find(|resource| resource.logical_id == gate.logical_id)
                    .and_then(|resource| resource.properties_blob());
                let operation_id = stack
                    .last_operation()
                    .map(|operation| operation.operation_id.clone());
                let client_request_token = stack.client_request_token.clone();
                // The signal is reported on the resource it was sent to, worded as every
                // signal this service records is ([`crate::state::wait_conditions`]), while
                // the resource is still the one being created.
                stack.record_resource_event(
                    now,
                    &gate.logical_id,
                    Some(gate.physical_resource_id.clone()),
                    &gate.resource_type,
                    crate::state::resource_status::CREATE_IN_PROGRESS,
                    Some(crate::state::wait_conditions::signal_received(
                        crate::state::resource_signal_status::SUCCESS,
                        instance_id,
                    )),
                    properties,
                    operation_id,
                    client_request_token,
                );
            }
            // A wait the signals have now satisfied proceeds, which is what makes the
            // operation's terminal status due; a gate still short of its `Count` holds, and
            // this is a no-op for it.
            stack.release_wait_conditions(now);
        }
    }
}

/// The gates of one stack whose signals come from the resource's own instances: the
/// unsatisfied resource-signal waits of the CREATE in flight, for a resource of a type whose
/// instances signal and that the operation has already given a physical id.
///
/// A `CreationPolicy` "is invoked during resource creation", so only a create is read here;
/// every other wait — a wait condition, an update — is left to the client's own signals.
fn open_instance_gates(stack: &crate::state::StoredStack) -> Vec<InstanceGate> {
    if stack.pending.is_none() || stack.status != crate::state::status::CREATE_IN_PROGRESS {
        return Vec::new();
    }
    let resources = stack.current_resources();
    stack
        .wait_conditions
        .iter()
        .filter(|barrier| !stack.barrier_satisfied(barrier))
        .filter_map(|barrier| {
            let resource = resources
                .iter()
                .find(|resource| resource.logical_id == barrier.logical_id())?;
            if resource.physical_resource_id.is_empty() {
                return None;
            }
            Some(InstanceGate {
                logical_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                physical_resource_id: resource.physical_resource_id.clone(),
                signalling: Signalling::of(&resource.resource_type)?,
                needed: barrier
                    .count()
                    .saturating_sub(stack.successful_signals(barrier.logical_id())),
                signalled_already: stack
                    .signals
                    .iter()
                    .filter(|signal| {
                        signal.logical_resource_id == barrier.logical_id()
                            && signal.status == crate::state::resource_signal_status::SUCCESS
                    })
                    .map(|signal| signal.unique_id.clone())
                    .collect(),
            })
        })
        .collect()
}

/// The ids of the instances the gate's resource consists of that its owning service reports
/// SERVING right now.
///
/// An instance that is not up has not signalled: a group reports its serving instances as the
/// ones in service in it, and an instance of its own is serving once Amazon EC2 reports it
/// running — which is the same answer the stack holds that resource's own creation open until
/// ([`ec2_instance::readiness`]).
async fn serving_instances(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    gate: &InstanceGate,
) -> Result<Vec<String>, Refusal> {
    match gate.signalling {
        Signalling::AutoScalingGroup => {
            crate::state::provisioner::compute_fleet::in_service_instance_ids(
                ctx,
                region,
                &gate.physical_resource_id,
            )
            .await
        }
        Signalling::Instance => {
            match ec2_instance::readiness(ctx, region, &gate.physical_resource_id).await? {
                Readiness::Ready => Ok(vec![gate.physical_resource_id.clone()]),
                // Still coming up, or in a state no instance serves from: it has signalled
                // nothing.
                Readiness::Pending(_) | Readiness::Failed(_) => Ok(Vec::new()),
            }
        }
    }
}

/// The `cfn-signal` helper script, which is how AWS documents an instance signalling the
/// stack that created it: "you can use the `cfn-signal` helper script … to signal".
const SIGNAL_HELPER_SCRIPT: &str = "cfn-signal";

/// The `SignalResource` API as a command line spells it (`aws cloudformation
/// signal-resource`), which is the other way a booting instance signals its stack — the one
/// "for … signals from anywhere other than an Amazon EC2 instance", reached from an instance
/// that has the CLI rather than the helper scripts.
const SIGNAL_RESOURCE_COMMAND: &str = "signal-resource";

/// WHICH status the boot configuration asks the instance to signal `logical_id` with, or
/// `None` where it asks it for no signal on that resource at all.
///
/// A bootstrap runs more than one command — `cfn-init` before `cfn-signal` is the pattern
/// `CreationPolicy` itself documents — so what is read here is each INVOCATION of a
/// signalling command on its own ([`signal_invocations`]), and the options of one invocation
/// say nothing about another.
///
/// Both ways of signalling name the resource they signal — the helper script as
/// `--resource <LogicalId>`, the API's command as `--logical-resource-id <LogicalId>` — so an
/// invocation that names ANOTHER resource outright is not this gate's signal. One whose
/// argument is a value the instance builds at boot (`--resource ${LogicalId}`, `-r
/// $RESOURCE`) names no resource this read can resolve, and the instances are the gate
/// resource's own, which is what makes their signal its own.
///
/// "The status of the signal, which is either success or failure" (`SignalResource`), and an
/// invocation states which it asks for: the API's own command states it as
/// `--status <STATUS>`, and the helper script states it as the exit code it reports
/// (`cfn-signal -e 0` / `--exit-code 0` is a bootstrap that succeeded, a non-zero one its
/// failure) or as `--success <true|false>`. A configuration that invokes neither way of
/// signalling asks for no signal, one whose invocation for this resource states a failure asks
/// for that, and one that invokes it without stating a failure asks for the success of an
/// instance that came up — which is what the owning service reporting that instance serving
/// says happened.
///
/// The two ways of asking are AWS's own, and the options are read as the words those commands
/// publish, so a value a template builds at boot (`-e $?`) states no failure here: what
/// failed such a bootstrap is work this emulator did not run.
fn asked_signal_status(boot_configuration: &str, logical_id: &str) -> Option<&'static str> {
    let asked = decoded(boot_configuration);
    let mut asked_failure = None;
    for invocation in signal_invocations(&asked) {
        // The words of THAT invocation: an option joined to its value by `=` is the pair it
        // is, which is why the separator counts as one here.
        let tokens: Vec<&str> = invocation
            .split(|character: char| character.is_whitespace() || character == '=')
            .filter(|token| !token.is_empty())
            .collect();
        if names_another_resource(&tokens, logical_id) {
            continue;
        }
        if asks_for_failure(&tokens) {
            asked_failure = Some(crate::state::resource_signal_status::FAILURE);
            continue;
        }
        return Some(crate::state::resource_signal_status::SUCCESS);
    }
    asked_failure
}

/// Every INVOCATION of a signalling command the boot configuration carries: the text from the
/// command to the end of the command line it is on, so the options read for one invocation are
/// its own.
///
/// A command line ends at a newline or at one of the separators a shell starts another command
/// at.
///
/// Bounded in both directions, the configuration being a value a caller stored: the text read
/// is at most one configuration's documented size ([`decoded`]), each step advances past the
/// END of the invocation it took — so no byte is scanned twice and the invocations cannot
/// overlap — and each invocation is a SLICE of that text rather than a copy of it.
fn signal_invocations(asked: &str) -> Vec<&str> {
    const SEPARATORS: [char; 4] = ['\n', ';', '&', '|'];

    let mut invocations = Vec::new();
    let mut rest = asked;
    loop {
        let found = [SIGNAL_HELPER_SCRIPT, SIGNAL_RESOURCE_COMMAND]
            .iter()
            .filter_map(|command| rest.find(command))
            .min();
        let Some(at) = found else {
            return invocations;
        };
        let invocation = &rest[at..];
        let end = invocation.find(SEPARATORS).unwrap_or(invocation.len());
        invocations.push(&invocation[..end]);
        rest = &invocation[end..];
    }
}

/// Whether this invocation names a resource OTHER than `logical_id`: an argument it states
/// outright, rather than one the instance builds at boot (which carries a substitution and so
/// names no resource this read can resolve).
fn names_another_resource(tokens: &[&str], logical_id: &str) -> bool {
    tokens.windows(2).any(|pair| match pair {
        [option, value] => {
            matches!(*option, "-r" | "--resource" | "--logical-resource-id")
                && !value.contains('$')
                && *value != logical_id
        }
        _ => false,
    })
}

/// Whether this invocation asks for the FAILURE of the bootstrap it reports: the status the
/// API's command states, or the exit code — or the success flag — the helper script does.
fn asks_for_failure(tokens: &[&str]) -> bool {
    tokens.windows(2).any(|pair| match pair {
        [option, value] => {
            (option.eq_ignore_ascii_case("--status")
                && value.eq_ignore_ascii_case(crate::state::resource_signal_status::FAILURE))
                || (matches!(*option, "-e" | "--exit-code")
                    && value.parse::<i64>().is_ok_and(|code| code != 0))
                || (matches!(*option, "-s" | "--success") && value.eq_ignore_ascii_case("false"))
        }
        _ => false,
    })
}

/// The most of a boot configuration this read looks at: "the maximum size of the user data is
/// 16 KB" (`RunInstances$UserData`), so a configuration an instance really booted with is at
/// most that, and what a caller stored past it is not read here — the work one of these reads
/// costs is bounded whatever the stored value's size.
const MAX_BOOT_CONFIGURATION_BYTES: usize = 16 * 1024;

/// The boot configuration as TEXT, bounded at [`MAX_BOOT_CONFIGURATION_BYTES`]: "the user data
/// … is base64-encoded" on the wire (`Instance$UserData`), so the stored value is decoded —
/// and one the codec does not admit, or that is not text at all, is read as it was written
/// rather than dropped.
fn decoded(boot_configuration: &str) -> String {
    let mut text = ::aws_smithy_types::base64::decode(boot_configuration)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| boot_configuration.to_string());
    if text.len() > MAX_BOOT_CONFIGURATION_BYTES {
        // Cut at a character boundary at or below the bound, so the text stays the text it is
        // (a configuration of that size carries nothing this read is looking for anyway).
        let cut = (0..=MAX_BOOT_CONFIGURATION_BYTES)
            .rev()
            .find(|at| text.is_char_boundary(*at))
            .unwrap_or(0);
        text.truncate(cut);
    }
    text
}
