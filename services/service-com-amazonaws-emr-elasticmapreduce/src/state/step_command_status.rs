// SPDX-License-Identifier: CC0-1.0

//! What a step's COMMAND answered: the exit status of the command a `command-runner.jar`
//! step submits, which is what that step ends in.
//!
//! # NO host command, process or shell is ever started
//!
//! Nothing in this module — and nothing it calls — spawns a process, invokes a shell, opens a
//! socket or touches a file. There is no `std::process::Command`, no `exec`, no `sh -c` and no
//! host binary lookup anywhere on this path, and the one maintained shell ENGINE that could
//! provide them, `brush-core`, is deliberately not a dependency of this workspace (see
//! [`::service_common::posix_shell_session`], which states why).
//!
//! What a step's command reaches instead is a PARSE and a fixed allow-list. The command's text
//! is parsed into a syntax tree by `brush-parser` — a library that builds a tree and runs
//! nothing — and the status is answered by that session's catalog of SESSION-LOCAL utilities,
//! whose whole effect is inside the session itself: `echo` (a line of the session's own
//! output, which a step keeps none of), `true`, `false` and `:` (a status), `exit` (a status,
//! ending the session) and `export`/`unset` (the session's own variables). Every command
//! outside that allow-list, and every command FORM or expansion the session does not perform,
//! is REFUSED with the status a shell gives a command it could not find, and the step then
//! reports that its work was not run ([`STEP_NOT_EXECUTED_REASON`]) — never a status nothing
//! produced. Nothing a caller can write makes that list longer.
//!
//! # The rule this module implements
//!
//! "Amazon EMR executes each step in the order listed. For a step to be considered complete,
//! the main function must exit with a zero exit code" (`AddJobFlowSteps` doc), so the
//! terminal `StepState` of a step IS the exit status of what it submitted: zero is
//! `COMPLETED` and any other status is `FAILED`. For a `command-runner.jar` step the thing
//! that exits is a COMMAND — "command-runner.jar … lets you run commands on your cluster"
//! (Amazon EMR documentation), so the step's arguments ARE the command line — and that status
//! is answered here rather than assumed.
//!
//! Two forms of command line are read, and WHICH form a step submitted is read off its
//! arguments rather than out of their text: a line whose first word names a shell with `-c`
//! ([`SHELL_COMMAND_NAMES`]) carries the shell PROGRAM in the word after it, which is the form
//! a step wraps a whole command line in; every other line is the utility its first word names
//! with the words after it as its arguments, rendered back into one line by the maintained
//! shell quoter so that the session reads back exactly those words.
//!
//! # Bounds
//!
//! The session's own — the command budget, the line size, the nesting count, the expansion
//! sizes and the variable store — are stated in that module and live in code the command
//! cannot reach; it is handed no environment of this service's or of the host's. WHEN a
//! command is answered for, and how many of them one call answers for, is the queue side's
//! ([`super::command_step_run`]).
//!
//! # What is NOT answered here
//!
//! The two classes of submission this service really runs — a `spark-submit` of a Python
//! application ([`super::spark_step_run`]) and a read-only statement resolvable in the
//! cluster's Hive metastore ([`super::step_runner`]) — answer for themselves, so a step
//! submitting one of them is never read as a command.

use ::service_common::posix_shell_session::{SessionConfig, ShellSession};

use super::*;

/// The shells a step's command line carries a PROGRAM for, by the name it names one under.
///
/// SOURCE: POSIX's `sh -c command_string` ("read commands from the `command_string`
/// operand"), and the Bourne-compatible shells that spell the same option the same way. A
/// command line whose first word names one of these with [`SHELL_PROGRAM_OPTION`] carries a
/// shell program in the word after it, which is the text that is read; a line naming anything
/// else is the utility it names. Naming one of these starts nothing: no shell of the host is
/// ever looked up or invoked (see this module's header).
const SHELL_COMMAND_NAMES: [&str; 4] = ["sh", "bash", "dash", "ash"];

/// The option a shell reads its program from, as POSIX spells it.
const SHELL_PROGRAM_OPTION: &str = "-c";

/// How many words of a step's command line this reading walks. CHOSEN: a bound on what one
/// step's command can cost to read, far above any real command line.
const MAX_COMMAND_LINE_WORDS: usize = 512;

/// `FailureDetails$Reason` of a step whose command exited with a non-zero status: the class of
/// the failure, beside the status itself as the message ([`command_failure_message`]).
pub const COMMAND_FAILED_REASON: &str =
    "The command the step submitted exited with a non-zero status.";

/// `FailureDetails$Message` of such a step: "the descriptive message including the error the
/// Amazon EMR service has identified as the cause of step failure"
/// (`FailureDetails$Message` doc), which here is the status the command exited with.
#[must_use]
pub fn command_failure_message(status: i32) -> String {
    format!("The command the step submitted exited with status {status}.")
}

/// The command a `command-runner.jar` step submits, in the form it submitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepCommand {
    /// A shell PROGRAM: the command line named a shell with `-c`, and this is the text after
    /// it.
    Program(String),
    /// A utility and its arguments, as the command line wrote them, with no shell of its own
    /// to expand or split them.
    Argv(Vec<String>),
}

impl StepCommand {
    /// The command the step's arguments are, or `None` when the step submits none: its jar
    /// is not `command-runner.jar`, so its arguments are that jar's own and not a command
    /// line, or the command line is empty.
    ///
    /// A shell is recognised by the NAME it is named under, however the command line spelled
    /// its path, and its `-c` is read only while the shell is still reading options — a `-c`
    /// standing after the first word that is no option belongs to the command the shell was
    /// given, not to the shell.
    #[must_use]
    pub fn of(jar: &str, args: &[String]) -> Option<Self> {
        if !names_command_runner(jar) {
            return None;
        }
        let words: Vec<String> = args.iter().take(MAX_COMMAND_LINE_WORDS).cloned().collect();
        let first = words.first()?;
        let utility = first.split('/').next_back().unwrap_or(first);
        if SHELL_COMMAND_NAMES.contains(&utility) {
            for (index, word) in words.iter().enumerate().skip(1) {
                if !word.starts_with('-') {
                    break;
                }
                if word == SHELL_PROGRAM_OPTION {
                    // `sh [option...] -c <program> [name [argument...]]`: the word after the
                    // option is the program. A `-c` with nothing after it names no program,
                    // and the line is then the shell invocation it is.
                    if let Some(program) = words.get(index + 1) {
                        return Some(Self::Program(program.clone()));
                    }
                    break;
                }
            }
        }
        Some(Self::Argv(words))
    }

    /// This command as the one line the session reads.
    ///
    /// A shell program is read as it was written. A utility and its arguments are rendered
    /// back into a line by the maintained shell quoter, so the words the session reads back
    /// out of it are exactly the words the step submitted.
    fn as_line(&self) -> String {
        match self {
            Self::Program(program) => program.clone(),
            Self::Argv(words) => ::shell_words::join(words),
        }
    }
}

/// What this command ANSWERED, or `None` when the session answered for nothing of it.
///
/// The status is the session's ([`answered_status`]), and the step's end follows from it the
/// way `AddJobFlowSteps` documents: a zero status is the step's completion, and any other
/// status is the modelled failure state carrying the status in both members the model gives a
/// step failure.
#[must_use]
pub fn command_outcome(command: &StepCommand) -> Option<StepOutcome> {
    let status = answered_status(command)?;
    if status == 0 {
        return Some(StepOutcome {
            state: step_state::COMPLETED.to_string(),
            failure: None,
        });
    }
    Some(StepOutcome {
        state: step_state::FAILED.to_string(),
        failure: Some(StepFailure {
            reason: COMMAND_FAILED_REASON.to_string(),
            message: command_failure_message(status),
        }),
    })
}

/// The status the session answered for `command`, or `None` when it answered for nothing of
/// it.
///
/// The session holds the parse and the allow-list this module's header states, and nothing
/// else: no process is started here. It is given no variables either — a step's command is
/// handed no environment, so a parameter it expands is one the command itself set. The one
/// status that is not an answer about the command is the one a shell gives a utility it could
/// not find, which is what the session answers for every command outside its catalog — unless
/// the program ENDED the session itself, which is a status the command really asked for.
fn answered_status(command: &StepCommand) -> Option<i32> {
    let config = SessionConfig::default();
    let mut session = ShellSession::new(BTreeMap::new(), config);
    let status = session.run(&command.as_line());
    if status == config.statuses.could_not_run && session.exit_status().is_none() {
        return None;
    }
    Some(status)
}
