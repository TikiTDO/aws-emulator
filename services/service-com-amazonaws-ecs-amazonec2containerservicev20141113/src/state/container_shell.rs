// SPDX-License-Identifier: CC0-1.0

//! What a container's COMMAND does: the session one container's program runs in.
//!
//! # The rule this module implements
//!
//! A container definition's `command` is "the command that's passed to the container", and
//! `entryPoint` the "entry point that's passed to the container"; together they are the
//! argument vector the container's image runs. What the container WRITES on its standard
//! output and the STATUS it exits with are therefore produced by running that vector —
//! `Container$exitCode` is "the exit code returned from the container" — and so is the
//! container's reaction to the signal `StopTask` sends it ("the equivalent of docker stop
//! is issued ... This results in a stop signal value ... will default to SIGTERM. If the
//! container handles the SIGTERM value gracefully and exits within 30 seconds from
//! receiving it, no SIGKILL value is sent").
//!
//! The grammar is not read here: a shell program is tokenized and parsed by `brush-parser`,
//! the POSIX/bash tokenizer and grammar, exactly as this workspace reads caller-supplied
//! SQL with `sqlparser` and caller-supplied YAML with `yaml-rust2`. That parser builds a
//! syntax tree and runs nothing, so the and-or lists, pipelines, loops, conditionals,
//! assignments and quoting of a program are the shell's own rather than something scanned
//! out of the text. Word expansion is read from the same parser's word pieces.
//!
//! # Why the grammar and not an engine
//!
//! The one maintained shell ENGINE this grammar has — `brush-core`, the executing half of
//! the same project — is deliberately not a dependency of this crate, and what follows is
//! not a reimplementation of it. An engine's whole purpose is to run what the shell
//! language can express: it resolves a command name against the host's `PATH` and starts a
//! host PROCESS for it, opens the host files a redirection names, and expands a command
//! substitution by running that command. Those are exactly the primitives this module has
//! none of, and the crate exposes no seam to replace them with an in-memory volume or an
//! in-process dispatcher, so there is no configuration of it that could be confined here:
//! taking it would hand a caller's task definition the developer's machine.
//!
//! What this module does instead is narrower than a shell by construction, and it is not an
//! implementation of the shell LANGUAGE: it PERFORMS a fixed catalog of session-local
//! utilities ([`BUILT_IN_COMMANDS`]) over the real grammar's tree, and every other command,
//! and every command FORM and expansion it does not perform, fails closed with the status a
//! shell answers for a command it cannot run ([`EXIT_COMMAND_NOT_FOUND`]). There is
//! therefore no language subset to keep in step with a specification: a command either
//! names one of those utilities, or it did not run.
//!
//! # The other reader of this grammar
//!
//! `service-com-amazonaws-codebuild-codebuild-20161006`'s `state::build_shell` reads a
//! BUILDSPEC's command lines with the same parser crate, and the two are deliberately not
//! one module: what is shared is the grammar, which is the crate, and what differs is the
//! whole of the rest. A build's shell is synchronous and performs one command LINE at a
//! time for a caller that is waiting on a status, keeps no output at all, has no signals
//! and no waits, and answers the compound forms as commands it did not run. A container's
//! is a PROCESS: it runs a whole program without a caller waiting on it, delivers every
//! line it writes as it writes it, waits, loops, and answers the stop signal through the
//! handler the program installed. Merging them would mean giving a buildspec's line the
//! container's loops, waits and signal handling — a widening of what a build may run,
//! decided in a change about neither.
//!
//! # What this container provides
//!
//! It runs IN THIS PROCESS. It has no process to start, no network to reach, no filesystem
//! and no host environment: the utilities it provides are the ones whose whole effect is
//! inside the container's own session — its output, its status, its variables and its
//! signal handlers. The variables it starts with are the container definition's
//! `environment` (and a `RunTask` override of it), because those are the environment the
//! container's process is given.
//!
//! An asynchronous list (`command &`) is run in the FOREGROUND: this session has no job
//! control, so there is no second thread of execution for a job to run on, and `wait`
//! answers success because no job is outstanding.
//!
//! # Bounds
//!
//! Every bound is this emulator's, held in code the program cannot reach: how many STEPS
//! one run may take ([`MAX_COMMANDS_PER_RUN`], one per command the walk visits — performed
//! or declined — and one per loop iteration, latched: once spent the run is ABANDONED and no
//! status is claimed for it), how long it may run for (the deadline its session is
//! constructed with, which also clamps every wait), how deeply a line may nest
//! ([`MAX_COMMAND_NESTING`]), how much one word may expand to, and how much output it may
//! deliver ([`super::MAX_CONTAINER_LOG_LINES`]).

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;

/// The exit status of a command that ran to success.
const EXIT_SUCCESS: i32 = 0;

/// The exit status of a command that ran and answered failure, which is what the `false`
/// utility is specified to do ("exit with a status code indicating failure").
const EXIT_FAILURE: i32 = 1;

/// The exit status a shell reports for a program it could not parse — the status `sh` is
/// specified to exit with on a syntax error it detects.
const EXIT_SYNTAX_ERROR: i32 = 2;

/// The exit status a shell reports when "the utility to be executed could not be found",
/// which is what this container answers for every command it provides no program for, and
/// for a command form it does not perform.
const EXIT_COMMAND_NOT_FOUND: i32 = 127;

/// What a shell adds to a signal number to report the status of a command a signal ended:
/// "the exit status shall be 128 plus the signal number".
const EXIT_SIGNAL_BASE: i32 = 128;

/// The number of the termination signal — `SIGTERM`, the signal `StopTask` has the agent
/// send a container ("will default to SIGTERM") — so a session with no handler for it ends
/// with [`EXIT_SIGNAL_BASE`] plus this.
const SIGTERM_NUMBER: i32 = 15;

/// How `trap` names the termination signal, and the spellings a program may write it as:
/// the bare name, the `SIG`-prefixed name and the number.
const SIGTERM_NAMES: [&str; 3] = ["TERM", "SIGTERM", "15"];

/// The commands this container provides, as their names.
///
/// Each one's whole effect is inside the container's own session, which is why it can be
/// performed in process: `echo` writes to the container's output, `true`, `false` and `:`
/// answer a status, `exit` ends the session, `export`/`unset` change its variables, `trap`
/// records what a signal does to it, `sleep` waits, and `wait` answers for the jobs it has
/// none of. A command that is not one of these needs a program, a filesystem or a network
/// this container does not have.
pub const BUILT_IN_COMMANDS: &[&str] = &[
    "echo", "true", "false", ":", "exit", "export", "unset", "trap", "sleep", "wait",
];

/// The commands an assignment written before them persists in the session for: POSIX's
/// special built-in utilities, for which "variable assignments specified with special
/// built-in utilities remain in effect after the built-in completes".
const PERSISTING_COMMANDS: &[&str] = &[":", "exit", "export", "unset", "trap"];

/// The shells this session reads a `-c` PROGRAM from, by the name they are invoked under.
///
/// A container's argument vector is exec'd, not interpreted, so a vector whose first word
/// is one of these with a `-c` carries a shell program in the word after it — the form a
/// task definition writes a script in — while every other vector names a utility and its
/// arguments.
const SHELL_COMMAND_NAMES: [&str; 4] = ["sh", "bash", "dash", "ash"];

/// How many STEPS one container run may take — one per command the walk visits and one per
/// loop iteration. THIS NUMBER IS THE EMULATOR'S, not a quota of the service: the run
/// happens in this process, so without a ceiling a program written as an endless loop of
/// instant commands would hold a thread of this emulator for as long as the process lives.
/// Once it is spent the run is ABANDONED — no status is claimed for the container — rather
/// than reported as having ended.
pub const MAX_COMMANDS_PER_RUN: usize = 200_000;

/// How many steps this session takes between handing the runtime back. THIS NUMBER IS THE
/// EMULATOR'S: a program whose commands never wait would otherwise hold the executor thread
/// it runs on for its whole budget.
const COMMANDS_PER_YIELD: usize = 64;

/// The longest program text this session reads. THIS NUMBER IS THE EMULATOR'S, for the same
/// reason as [`MAX_COMMANDS_PER_RUN`]: it bounds what one parse can cost, and is far above
/// any real container command.
const MAX_PROGRAM_BYTES: usize = 64 * 1024;

/// How many grouping indicators one program may carry before this session gives up on
/// reading it. THIS NUMBER IS THE EMULATOR'S, and it is what keeps the grammar's own
/// recursion in range: a parser descends once per nested group, so a program nested
/// arbitrarily deep would exhaust the stack of the thread running it. It is checked WITHOUT
/// parsing.
const MAX_COMMAND_NESTING: usize = 64;

/// The characters that open a nested group in a program, counted by
/// [`within_parse_bounds`]: a subshell or command substitution, a brace group or expansion,
/// a bracket expression, and the older command-substitution spelling.
const NESTING_INDICATORS: [char; 4] = ['(', '{', '[', '`'];

/// The reserved WORDS that open or close a nested command, counted by
/// [`within_parse_bounds`] beside [`NESTING_INDICATORS`]: the shell's compound commands
/// nest through these rather than through any character. The words that CLOSE one are
/// counted too, which can only make the count higher — an over-count refuses a program and
/// never admits one.
const NESTING_WORDS: [&str; 16] = [
    "if", "then", "elif", "else", "fi", "while", "until", "for", "select", "case", "esac", "in",
    "do", "done", "function", "time",
];

/// The characters a reserved word may be glued to: the shell's own separators and
/// operators. A word is counted by [`within_parse_bounds`] when it stands between any of
/// these (or whitespace), so `:;if` carries an `if` exactly as `: ; if` does.
const WORD_SEPARATORS: [char; 10] = [';', '&', '|', '(', ')', '{', '}', '<', '>', '!'];

/// How many bytes one expansion may produce before this session gives up on the word. THIS
/// NUMBER IS THE EMULATOR'S: a parameter expansion copies a value this session holds, so a
/// word that expands several of them is how one command could grow what the next one copies
/// again.
const MAX_EXPANSION_BYTES: usize = 64 * 1024;

/// How many bytes of variables one run's session may hold at once. THIS NUMBER IS THE
/// EMULATOR'S, under [`MAX_EXPANSION_BYTES`]' rule applied to the session rather than to one
/// word: past it a command that would set another variable is not performed.
const MAX_SESSION_BYTES: usize = 1024 * 1024;

/// How many variables one run's session may hold at once. THIS NUMBER IS THE EMULATOR'S:
/// [`MAX_SESSION_BYTES`] bounds the store's SIZE and this bounds its cardinality.
const MAX_SESSION_VARIABLES: usize = 4096;

/// How many bytes ONE command's expansions may materialise at once. THIS NUMBER IS THE
/// EMULATOR'S: [`MAX_EXPANSION_BYTES`] bounds one word and this bounds their sum.
const MAX_LINE_EXPANSION_BYTES: usize = 256 * 1024;

/// How deeply a word's expansion is walked before this session gives up on it. THIS NUMBER
/// IS THE EMULATOR'S, under [`MAX_COMMAND_NESTING`]'s rule, applied to the pieces a quoted
/// word nests.
const MAX_WORD_DEPTH: usize = 32;

/// How many signal handlers one session may hold. THIS NUMBER IS THE EMULATOR'S: a `trap`
/// records text, so without a ceiling a program could grow this session through it. Far
/// above the number of signals there are.
const MAX_TRAPS: usize = 64;

/// How many bytes of output this session holds before the line it is assembling is
/// delivered unterminated. THIS NUMBER IS THE EMULATOR'S: a program that writes without
/// ever ending a line would otherwise grow this buffer without end.
const MAX_PENDING_LINE_BYTES: usize = 256 * 1024;

/// A future this session's own recursion goes through, so the mutually recursive walk over
/// the grammar's tree has a size the compiler can work out.
type ShellFuture<'a, T> = Pin<Box<dyn ::std::future::Future<Output = T> + Send + 'a>>;

/// What one container is given to run: the argument vector its image was handed, read as
/// the program it is.
#[derive(Debug, Clone)]
pub enum ContainerProgram {
    /// A shell program: the vector named a shell with `-c`, and this is the text after it.
    Script(String),
    /// A utility and its arguments: the vector is exec'd as written, with no shell to
    /// expand or split anything.
    Argv(Vec<String>),
}

impl ContainerProgram {
    /// The program the argument vector `entry_point` followed by `command` runs, or `None`
    /// when the launch named no vector at all — a container whose image's own entry point
    /// is what would run, which this emulator does not hold and therefore does not invent.
    ///
    /// `entryPoint` comes first because that is the order the container runtime exec's
    /// them: "the entry point ... command is passed as the arguments to the entry point".
    ///
    /// A `-c` is the shell's OPTION only while the shell is still reading options: a vector
    /// is scanned from the shell's name while its words are options, so a `-c` standing
    /// after an operand is an argument of whatever that vector runs rather than this
    /// session's program selector — the same place the shell itself stops reading options.
    pub fn resolve(entry_point: Option<&[String]>, command: Option<&[String]>) -> Option<Self> {
        let mut argv: Vec<String> = Vec::new();
        argv.extend(entry_point.unwrap_or_default().iter().cloned());
        argv.extend(command.unwrap_or_default().iter().cloned());
        let first = argv.first()?;
        let utility = first.rsplit('/').next().unwrap_or(first);
        if SHELL_COMMAND_NAMES.contains(&utility) {
            // `sh [option...] -c <program> [name [argument...]]`: the word after `-c` is
            // the program, and the words after it are its positional parameters, which this
            // session does not provide.
            for (index, word) in argv.iter().enumerate().skip(1) {
                if !word.starts_with('-') || word == "--" {
                    break;
                }
                if word == "-c" {
                    if let Some(text) = argv.get(index + 1) {
                        return Some(Self::Script(text.clone()));
                    }
                    // A `-c` with nothing after it names no program: the vector is left as
                    // the one the image exec's, which this container has no program for,
                    // rather than read as a launch that named no command at all.
                    break;
                }
            }
        }
        Some(Self::Argv(argv))
    }
}

/// What a container's run ended as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerOutcome {
    /// The program ended on its own, with this status — the container's `exitCode`.
    Exited(i32),
    /// The run was given up on before the program ended, because a bound of this emulator
    /// was spent (its command budget or its deadline). NOTHING is claimed about the
    /// container: it reports no exit code, because none was produced.
    Abandoned,
}

/// Whether `text` is inside the bounds the parse below is given: its size, and how many
/// nested commands it opens — through a grouping character or a reserved word.
fn within_parse_bounds(text: &str) -> bool {
    if text.len() > MAX_PROGRAM_BYTES {
        return false;
    }
    let grouped = text
        .chars()
        .filter(|character| NESTING_INDICATORS.contains(character))
        .count();
    let worded = text
        .split(|character: char| {
            character.is_whitespace() || WORD_SEPARATORS.contains(&character)
        })
        .filter(|word| NESTING_WORDS.contains(word))
        .count();
    grouped.saturating_add(worded) <= MAX_COMMAND_NESTING
}

/// The session ONE container's program runs in: the variables it sees and changes, the
/// signal handlers it has installed, where its output goes, and how much running it has
/// left.
#[derive(Debug)]
pub struct ContainerShell {
    /// The variables this session holds — the container's environment — which is what a
    /// word's parameter expansion resolves against.
    variables: BTreeMap<String, String>,
    /// How many bytes those variables' names and values come to, kept as they are written
    /// rather than recomputed, so the [`MAX_SESSION_BYTES`] check costs one addition.
    held_bytes: usize,
    /// What a signal does to this session: the `trap` action recorded for each signal, by
    /// the name [`signal_key`] normalises it to. An empty action is a signal the program
    /// asked to IGNORE.
    traps: BTreeMap<String, String>,
    /// How many more bytes the command being run may expand to
    /// ([`MAX_LINE_EXPANSION_BYTES`]), reset for each command.
    expansion_left: usize,
    /// How many more commands this run may perform ([`MAX_COMMANDS_PER_RUN`]).
    commands_left: usize,
    /// How many commands have been performed since the runtime was last handed back
    /// ([`COMMANDS_PER_YIELD`]).
    commands_since_yield: usize,
    /// The status `exit` ended this session with, once a command asked for one — or the
    /// status a signal ended it with: every later command is then not run at all, the way a
    /// shell stops at `exit`.
    exit_status: Option<i32>,
    /// Whether a bound of this emulator ended the run before the program did, which is what
    /// makes the outcome [`ContainerOutcome::Abandoned`].
    abandoned: bool,
    /// Whether the termination signal has arrived and its handler has not run yet.
    pending_term: bool,
    /// Whether the termination handler is running, so the checkpoint inside it does not
    /// deliver the same signal to itself again.
    delivering_term: bool,
    /// The output the container writes to, and the `awslogs` delivery behind it.
    output: Arc<ContainerOutput>,
    /// The line being assembled: output is delivered one LINE at a time, so a write that
    /// does not end in a newline waits here for the one that does
    /// ([`MAX_PENDING_LINE_BYTES`]).
    pending_line: String,
    /// The signals this container can be sent — the flag `StopTask` raises.
    signals: Arc<ContainerSignals>,
    /// When this emulator gives up on the run.
    deadline: Instant,
}

impl ContainerShell {
    /// A session holding `variables` — the container's environment — writing to `output`,
    /// answering the signals raised on `signals`, and given until `deadline`.
    pub fn new(
        variables: BTreeMap<String, String>,
        output: Arc<ContainerOutput>,
        signals: Arc<ContainerSignals>,
        deadline: Instant,
    ) -> Self {
        let held_bytes = variables
            .iter()
            .map(|(name, value)| name.len().saturating_add(value.len()))
            .sum();
        Self {
            variables,
            held_bytes,
            traps: BTreeMap::new(),
            expansion_left: MAX_LINE_EXPANSION_BYTES,
            commands_left: MAX_COMMANDS_PER_RUN,
            commands_since_yield: 0,
            exit_status: None,
            abandoned: false,
            pending_term: false,
            delivering_term: false,
            output,
            pending_line: String::new(),
            signals,
            deadline,
        }
    }

    /// Run `program` to its end and answer what the container's run ended as.
    ///
    /// The status is the one the program ended with: `exit`'s, the last command's, or —
    /// for a session the termination signal ended with no handler installed for it — the
    /// status a shell reports for a command that signal ended ([`EXIT_SIGNAL_BASE`] plus
    /// the signal's number). A run a bound of this emulator ended instead is
    /// [`ContainerOutcome::Abandoned`], which claims nothing about the container.
    pub async fn run(&mut self, program: &ContainerProgram) -> ContainerOutcome {
        let mut status = match program {
            ContainerProgram::Script(text) => self.run_text(text).await,
            ContainerProgram::Argv(argv) => self.run_argv(argv).await,
        };
        // A program that ends while the signal it installed a handler for is outstanding
        // still runs that handler: the signal arrived before the program ended.
        self.deliver_pending_signal().await;
        if let Some(latched) = self.exit_status {
            status = latched;
        }
        // The last line the program wrote may not have ended in a newline; the container's
        // output ends where the container does, so it is delivered as the line it is.
        self.flush_pending_line().await;
        if self.abandoned {
            ContainerOutcome::Abandoned
        } else {
            ContainerOutcome::Exited(status)
        }
    }

    /// Run one shell PROGRAM text and answer the status the shell answered for it.
    ///
    /// A program that does not parse is the shell's own syntax error
    /// ([`EXIT_SYNTAX_ERROR`]); one outside the bounds this parse is given
    /// ([`within_parse_bounds`]) is a program this container did not run at all, which is
    /// reported as a command it could not run rather than as one that succeeded.
    fn run_text<'a>(&'a mut self, text: &'a str) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            if !within_parse_bounds(text) {
                return EXIT_COMMAND_NOT_FOUND;
            }
            let Ok(tokens) = ::brush_parser::tokenize_str(text) else {
                return EXIT_SYNTAX_ERROR;
            };
            let options = ::brush_parser::ParserOptions::default();
            let Ok(program) = ::brush_parser::parse_tokens(&tokens, &options) else {
                return EXIT_SYNTAX_ERROR;
            };
            let mut status = EXIT_SUCCESS;
            for complete_command in &program.complete_commands {
                status = self.run_complete_command(complete_command).await;
                if self.stopped() {
                    return status;
                }
            }
            status
        })
    }

    /// Run one complete command of a program — the grammar's list of and-or lists — and
    /// answer the status of the last one that ran.
    fn run_complete_command<'a>(
        &'a mut self,
        command: &'a ::brush_parser::ast::CompleteCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let mut status = EXIT_SUCCESS;
            for item in &command.0 {
                status = self.run_and_or_list(&item.0).await;
                if self.stopped() {
                    return status;
                }
            }
            status
        })
    }

    /// Run one of the COMPOUND command forms this container performs.
    ///
    /// A brace group and a subshell are the list they hold; a `while`/`until` loop runs its
    /// body for as long as its condition says, and an `if` runs the branch its condition
    /// chose. The loop needs no bound of its own: every command it performs comes out of
    /// the run's command budget, and a loop that outlasts it abandons the run.
    ///
    /// A subshell's effects are NOT confined to a child session here: there is no process
    /// to fork, so a variable it sets is set in this one. Every other compound form —
    /// `for`, `case`, an arithmetic command — is one this container does not perform.
    fn run_compound_command<'a>(
        &'a mut self,
        compound: &'a ::brush_parser::ast::CompoundCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            match compound {
                ::brush_parser::ast::CompoundCommand::BraceGroup(group) => {
                    self.run_compound_list(&group.list).await
                }
                ::brush_parser::ast::CompoundCommand::Subshell(subshell) => {
                    self.run_compound_list(&subshell.list).await
                }
                ::brush_parser::ast::CompoundCommand::WhileClause(clause) => {
                    self.run_loop(clause, true).await
                }
                ::brush_parser::ast::CompoundCommand::UntilClause(clause) => {
                    self.run_loop(clause, false).await
                }
                _ => EXIT_COMMAND_NOT_FOUND,
            }
        })
    }

    /// Run a `while` (or `until`) loop: its condition, then its body for as long as that
    /// condition says — "the while loop shall continuously execute ... until the condition
    /// has a non-zero exit status", and the `until` loop until it has a zero one — and
    /// answer the status of the last body that ran ([`EXIT_SUCCESS`] for a loop whose body
    /// never ran, which is the status POSIX gives it).
    ///
    /// The loop is bounded by the run's own step budget and deadline: every ITERATION takes
    /// a step out of them, as does every command inside it, so a loop that outlasts either
    /// abandons the run rather than spinning for ever — including one whose condition and
    /// body are forms this container declines and so perform no command at all.
    fn run_loop<'a>(
        &'a mut self,
        clause: &'a ::brush_parser::ast::WhileOrUntilClauseCommand,
        while_zero: bool,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let mut status = EXIT_SUCCESS;
            loop {
                // The iteration itself takes a step, before anything in it is reached:
                // that is what bounds a loop whose condition and body are forms this
                // container declines, which reach no command of their own.
                if !self.take_command_budget().await {
                    return status;
                }
                let condition = self.run_compound_list(&clause.0).await;
                if self.stopped() {
                    return status;
                }
                let again = if while_zero {
                    condition == EXIT_SUCCESS
                } else {
                    condition != EXIT_SUCCESS
                };
                if !again {
                    return status;
                }
                status = self.run_compound_list(&clause.1.list).await;
                if self.stopped() {
                    return status;
                }
            }
        })
    }

    /// Run a compound LIST — the grammar's list of and-or lists inside a group, a loop body
    /// or a conditional — and answer the status of the last one that ran.
    fn run_compound_list<'a>(
        &'a mut self,
        list: &'a ::brush_parser::ast::CompoundList,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let mut status = EXIT_SUCCESS;
            for item in &list.0 {
                status = self.run_and_or_list(&item.0).await;
                if self.stopped() {
                    return status;
                }
            }
            status
        })
    }

    /// Run the argument vector of a container whose image exec's it directly: its first
    /// word is the utility and the rest are its arguments, with no shell to expand or split
    /// them.
    async fn run_argv(&mut self, argv: &[String]) -> i32 {
        let Some((name, arguments)) = argv.split_first() else {
            return EXIT_SUCCESS;
        };
        if !self.take_command_budget().await {
            return EXIT_COMMAND_NOT_FOUND;
        }
        self.run_built_in(name, arguments).await
    }

    /// Whether this session has stopped running commands: `exit` (or a signal) latched a
    /// status, or a bound of this emulator was spent.
    fn stopped(&self) -> bool {
        self.exit_status.is_some() || self.abandoned
    }

    /// Take one STEP out of this run's budget, hand the runtime back every
    /// [`COMMANDS_PER_YIELD`] steps, and answer whether the run may go on at all.
    ///
    /// Every command the walk visits takes a step, performed or declined
    /// ([`Self::run_command`]), and so does every iteration of a loop
    /// ([`Self::run_loop`]): that is what makes the budget, the deadline and the handback
    /// reachable from ANY program shape rather than only from one that keeps reaching a
    /// simple command — a loop whose condition and body are forms this container declines
    /// pays for its iterations exactly as one that performs them does.
    ///
    /// `false` ABANDONS the run: the budget or the deadline is spent, so the program did
    /// not end and nothing is claimed about the container's status.
    async fn take_command_budget(&mut self) -> bool {
        if self.commands_left == 0 || Instant::now() >= self.deadline {
            self.abandoned = true;
            return false;
        }
        self.commands_left -= 1;
        self.commands_since_yield += 1;
        if self.commands_since_yield >= COMMANDS_PER_YIELD {
            self.commands_since_yield = 0;
            ::tokio::task::yield_now().await;
        }
        true
    }

    /// Deliver the termination signal if it has arrived and its handler has not run yet.
    ///
    /// A shell runs a trapped signal's action between commands, and a session with no
    /// action recorded for the signal ends with the status that signal gives a command. An
    /// action recorded as the empty string is the program asking for the signal to be
    /// IGNORED, which is what `trap '' TERM` records.
    fn deliver_pending_signal(&mut self) -> ShellFuture<'_, ()> {
        Box::pin(async move {
            if !self.pending_term || self.delivering_term {
                return;
            }
            self.pending_term = false;
            match self.trap_action() {
                Some(action) if action.is_empty() => {}
                Some(action) => {
                    self.delivering_term = true;
                    // The handler runs with the latch clear: its own `exit` is what ends
                    // the session, and that is the status the container exits with.
                    self.run_text(&action).await;
                    self.delivering_term = false;
                }
                None => {
                    self.exit_status = Some(EXIT_SIGNAL_BASE + SIGTERM_NUMBER);
                }
            }
        })
    }

    /// The action this session has recorded for the termination signal, under any of the
    /// names a program may have written it as ([`SIGTERM_NAMES`]).
    fn trap_action(&self) -> Option<String> {
        SIGTERM_NAMES
            .iter()
            .find_map(|name| self.traps.get(*name))
            .cloned()
    }

    /// Wait for `duration` — or until the deadline, when it names none — ending early if a
    /// signal arrives, and answer whether one did.
    ///
    /// Every wait is clamped to what is left of this run's deadline, so no wait a program
    /// asks for outlives the bound this emulator gave the run.
    async fn wait_for(&mut self, duration: Option<Duration>) -> bool {
        let left = self.deadline.saturating_duration_since(Instant::now());
        let waiting = duration.map_or(left, |asked| asked.min(left));
        // The waiter is registered BEFORE the flag is read, so a signal raised between the
        // two is still what ends this wait.
        let signalled = self.signals.notified();
        if self.signals.terminating() {
            self.pending_term = true;
            return true;
        }
        let interrupted = ::tokio::time::timeout(waiting, signalled).await.is_ok();
        if interrupted || self.signals.terminating() {
            self.pending_term = true;
            return true;
        }
        if Instant::now() >= self.deadline {
            self.abandoned = true;
        }
        false
    }

    /// Run an and-or list: its first pipeline, then each later pipeline only when the
    /// operator before it says so — `&&` runs the next on a zero status, `||` on a non-zero
    /// one — and answer the status of the last pipeline that ran.
    fn run_and_or_list<'a>(
        &'a mut self,
        list: &'a ::brush_parser::ast::AndOrList,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let mut status = self.run_pipeline(&list.first).await;
            for next in &list.additional {
                if self.stopped() {
                    break;
                }
                match next {
                    ::brush_parser::ast::AndOr::And(pipeline) => {
                        if status == EXIT_SUCCESS {
                            status = self.run_pipeline(pipeline).await;
                        }
                    }
                    ::brush_parser::ast::AndOr::Or(pipeline) => {
                        if status != EXIT_SUCCESS {
                            status = self.run_pipeline(pipeline).await;
                        }
                    }
                }
            }
            status
        })
    }

    /// Run a pipeline and answer its status: "the exit status of a pipeline shall be the
    /// exit status of the last command", negated when the pipeline is written with `!`.
    ///
    /// Every command of the pipeline is run, in order, because each answers its own status;
    /// what one writes is read by the next, and this container connects no command's output
    /// to another's input, so nothing is passed along but the statuses.
    fn run_pipeline<'a>(
        &'a mut self,
        pipeline: &'a ::brush_parser::ast::Pipeline,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let mut status = EXIT_SUCCESS;
            for command in &pipeline.seq {
                status = self.run_command(command).await;
                if self.stopped() {
                    break;
                }
            }
            if pipeline.bang {
                status = if status == EXIT_SUCCESS {
                    EXIT_FAILURE
                } else {
                    EXIT_SUCCESS
                };
            }
            status
        })
    }

    /// Run one command of a pipeline: a SIMPLE command, or one of the compound forms this
    /// container performs ([`Self::run_compound_command`]).
    ///
    /// Every other form a shell grammar admits — a function definition, an extended test —
    /// is one this container does not perform, and a command that did not run answers as
    /// one that could not be run rather than as one that succeeded.
    fn run_command<'a>(
        &'a mut self,
        command: &'a ::brush_parser::ast::Command,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            // Every command takes a step out of the run's budget, whether this container
            // performs it or declines it: a program made only of forms it declines is
            // bounded exactly as one it performs is ([`Self::take_command_budget`]).
            if !self.take_command_budget().await {
                return EXIT_COMMAND_NOT_FOUND;
            }
            match command {
                ::brush_parser::ast::Command::Simple(simple) => {
                    self.run_simple_command(simple).await
                }
                ::brush_parser::ast::Command::Compound(compound, redirects) => {
                    // A redirection names a file this container has no filesystem for, so a
                    // command carrying one did not run.
                    if redirects.is_some() {
                        return EXIT_COMMAND_NOT_FOUND;
                    }
                    self.run_compound_command(compound).await
                }
                _ => EXIT_COMMAND_NOT_FOUND,
            }
        })
    }

    /// Run a simple command: its variable assignments, then the command its first word
    /// names with the words after it as its arguments.
    ///
    /// A command with no word at all is the assignment form — "the variable assignments
    /// shall affect the current execution environment". An assignment written BEFORE a
    /// command word affects that command's environment alone, and persists in the session
    /// only for the special built-in utilities POSIX keeps it for ([`PERSISTING_COMMANDS`]);
    /// since every word is expanded before the assignments are applied, a command that does
    /// not keep them sees the value either way. Any prefix or suffix item that is neither a
    /// word nor an assignment is a redirection or a substitution this container does not
    /// perform, and the command is then not run.
    fn run_simple_command<'a>(
        &'a mut self,
        simple: &'a ::brush_parser::ast::SimpleCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            // A signal that arrived while the command before this one ran is delivered
            // first: a shell runs a trapped action between commands.
            self.deliver_pending_signal().await;
            if self.signals.terminating() && !self.delivering_term {
                self.pending_term = true;
                self.deliver_pending_signal().await;
            }
            if self.stopped() {
                return self.exit_status.unwrap_or(EXIT_SUCCESS);
            }
            // Each command gets the whole expansion budget: the words of one command are
            // live at the same instant, and the words of the command before it are not.
            self.expansion_left = MAX_LINE_EXPANSION_BYTES;
            let mut assignments: Vec<(String, String)> = Vec::new();
            let mut words: Vec<String> = Vec::new();
            let prefix = simple.prefix.as_ref().map(|prefix| &prefix.0);
            let suffix = simple.suffix.as_ref().map(|suffix| &suffix.0);
            for item in prefix.into_iter().chain(suffix).flatten() {
                match item {
                    ::brush_parser::ast::CommandPrefixOrSuffixItem::Word(word) => {
                        let Some(expanded) = self.expand_word(&word.value) else {
                            return EXIT_COMMAND_NOT_FOUND;
                        };
                        words.push(expanded);
                    }
                    ::brush_parser::ast::CommandPrefixOrSuffixItem::AssignmentWord(
                        assignment,
                        _,
                    ) => {
                        let Some(pair) = self.assignment_of(assignment) else {
                            return EXIT_COMMAND_NOT_FOUND;
                        };
                        assignments.push(pair);
                    }
                    _ => return EXIT_COMMAND_NOT_FOUND,
                }
            }
            let name = match simple.word_or_name.as_ref() {
                Some(word) => match self.expand_word(&word.value) {
                    Some(expanded) => Some(expanded),
                    None => return EXIT_COMMAND_NOT_FOUND,
                },
                None => None,
            };
            let persists = name
                .as_deref()
                .is_none_or(|name| PERSISTING_COMMANDS.contains(&name));
            if persists {
                for (name, value) in assignments {
                    if !self.set_variable(name, value) {
                        return EXIT_COMMAND_NOT_FOUND;
                    }
                }
            }
            match name {
                Some(name) => self.run_built_in(&name, &words).await,
                // Nothing but assignments: they have been applied, which is the whole
                // command.
                None => EXIT_SUCCESS,
            }
        })
    }

    /// One assignment of a simple command as the name and value it sets, or `None` when it
    /// is a form this container does not perform — an array assignment, an appending
    /// assignment, or a value whose expansion it cannot resolve.
    fn assignment_of(
        &mut self,
        assignment: &::brush_parser::ast::Assignment,
    ) -> Option<(String, String)> {
        if assignment.append {
            return None;
        }
        let ::brush_parser::ast::AssignmentName::VariableName(name) = &assignment.name else {
            return None;
        };
        let ::brush_parser::ast::AssignmentValue::Scalar(value) = &assignment.value else {
            return None;
        };
        Some((name.clone(), self.expand_word(&value.value)?))
    }

    /// Set `name` to `value` in this session, unless it is already holding
    /// [`MAX_SESSION_BYTES`] or [`MAX_SESSION_VARIABLES`] — `false` is then a command this
    /// container did not perform, which is what its caller reports.
    fn set_variable(&mut self, name: String, value: String) -> bool {
        let displaced = self
            .variables
            .get(&name)
            .map(|held| name.len().saturating_add(held.len()));
        if displaced.is_none() && self.variables.len() >= MAX_SESSION_VARIABLES {
            return false;
        }
        let would_hold = self
            .held_bytes
            .saturating_sub(displaced.unwrap_or(0))
            .saturating_add(name.len())
            .saturating_add(value.len());
        if would_hold > MAX_SESSION_BYTES {
            return false;
        }
        self.held_bytes = would_hold;
        self.variables.insert(name, value);
        true
    }

    /// Remove `name` from this session, with the bytes it held.
    fn unset_variable(&mut self, name: &str) {
        if let Some(value) = self.variables.remove(name) {
            self.held_bytes = self
                .held_bytes
                .saturating_sub(name.len().saturating_add(value.len()));
        }
    }

    /// Run the command `name` with `arguments`, when it is one this container provides
    /// ([`BUILT_IN_COMMANDS`]).
    ///
    /// `echo` writes its arguments and a newline to the container's output (`-n` leaves the
    /// newline off, as that option is specified to). `true` and `:` answer success and
    /// `false` answers failure, which is the whole of what those utilities do. `exit` ends
    /// the session with the status its argument names, defaulting to success, and a status
    /// outside 0..=255 is taken modulo 256 the way a shell takes an exit status.
    /// `export`/`unset` change the session's variables, `trap` records what a signal does
    /// to it, `sleep` waits for the seconds its argument names, and `wait` answers success
    /// because this session starts no job to wait for.
    fn run_built_in<'a>(
        &'a mut self,
        name: &'a str,
        arguments: &'a [String],
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            // The catalog is what this container provides: a command outside it needs a
            // program, a filesystem or a network this container does not have.
            if !BUILT_IN_COMMANDS.contains(&name) {
                return EXIT_COMMAND_NOT_FOUND;
            }
            match name {
                "echo" => {
                    let (newline, printed) = match arguments.split_first() {
                        Some((first, rest)) if first == "-n" => (false, rest),
                        _ => (true, arguments),
                    };
                    let mut written = printed.join(" ");
                    if newline {
                        written.push('\n');
                    }
                    self.write_output(&written).await;
                    EXIT_SUCCESS
                }
                "true" | ":" => EXIT_SUCCESS,
                "false" => EXIT_FAILURE,
                "exit" => {
                    let status = match arguments.first() {
                        Some(argument) => match argument.parse::<i64>() {
                            Ok(status) => {
                                i32::try_from(status.rem_euclid(256)).unwrap_or(EXIT_FAILURE)
                            }
                            // A status a shell cannot read is its own usage failure, and
                            // the session still ends.
                            Err(_) => EXIT_FAILURE,
                        },
                        None => EXIT_SUCCESS,
                    };
                    self.exit_status = Some(status);
                    status
                }
                "export" => {
                    for argument in arguments {
                        // `export NAME=VALUE` sets it; `export NAME` marks a variable this
                        // session already holds, and marking one it does not hold sets
                        // nothing.
                        if let Some((name, value)) = argument.split_once('=') {
                            if !self.set_variable(name.to_string(), value.to_string()) {
                                return EXIT_COMMAND_NOT_FOUND;
                            }
                        }
                    }
                    EXIT_SUCCESS
                }
                "unset" => {
                    for argument in arguments {
                        self.unset_variable(argument);
                    }
                    EXIT_SUCCESS
                }
                "trap" => self.run_trap(arguments),
                "sleep" => self.run_sleep(arguments).await,
                // "wait: ... If no process ID is given, waits for all currently active
                // child processes" — this session has none, so there is nothing to wait
                // for and the utility answers success.
                "wait" => EXIT_SUCCESS,
                _ => EXIT_COMMAND_NOT_FOUND,
            }
        })
    }

    /// `trap ACTION SIGNAL...`: record what each named signal does to this session.
    ///
    /// The forms POSIX gives the utility: an action of `-` resets the signal to its default
    /// (which ends the session), an empty action has it IGNORED, and any other action is
    /// the program the session runs when the signal arrives.
    ///
    /// Two limits of this session, stated because a program cannot see them: a call naming
    /// NO signal records nothing and writes nothing — a shell would list the traps it holds
    /// on its output, and this writes no listing — and an action is recorded for whatever
    /// signal the program names while only the TERMINATION signal is ever raised here
    /// ([`ContainerSignals`]), so a handler for any other signal is recorded and never runs.
    fn run_trap(&mut self, arguments: &[String]) -> i32 {
        let Some((action, signals)) = arguments.split_first() else {
            return EXIT_SUCCESS;
        };
        if signals.is_empty() {
            return EXIT_SUCCESS;
        }
        for signal in signals {
            let key = signal_key(signal);
            if action == "-" {
                self.traps.remove(&key);
                continue;
            }
            if !self.traps.contains_key(&key) && self.traps.len() >= MAX_TRAPS {
                return EXIT_FAILURE;
            }
            self.traps.insert(key, action.clone());
        }
        EXIT_SUCCESS
    }

    /// `sleep SECONDS`: wait for the seconds its argument names, or until a signal arrives.
    ///
    /// The argument is read as a number of seconds, decimals included, and `infinity` is a
    /// wait with no end of its own — the way the utility's own implementations read it. A
    /// wait is clamped to what this run has left ([`Self::wait_for`]), so it never outlives
    /// the bound the emulator gave the run. An argument that is no duration at all is the
    /// utility's usage failure.
    async fn run_sleep(&mut self, arguments: &[String]) -> i32 {
        let Some(argument) = arguments.first() else {
            return EXIT_FAILURE;
        };
        let waiting = if argument == "infinity" || argument == "inf" {
            None
        } else {
            // `try_from_secs_f64` rather than the panicking constructor: the argument is a
            // caller's, so a value no duration can hold is the utility's usage failure
            // rather than this session's.
            match argument
                .parse::<f64>()
                .ok()
                .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
            {
                Some(waiting) => Some(waiting),
                None => return EXIT_FAILURE,
            }
        };
        if self.wait_for(waiting).await {
            // "If sleep receives a signal, it shall exit with a status greater than zero"
            // — the status a signal gives a command.
            return EXIT_SIGNAL_BASE + SIGTERM_NUMBER;
        }
        EXIT_SUCCESS
    }

    /// Write `text` to the container's output, delivering every COMPLETE line in it.
    ///
    /// The `awslogs` driver delivers one log event per line, so a write that does not end a
    /// line waits for the write that does — up to [`MAX_PENDING_LINE_BYTES`], past which
    /// what has been written is delivered as the line it is.
    async fn write_output(&mut self, text: &str) {
        for character in text.chars() {
            if character == '\n' {
                self.flush_pending_line().await;
                continue;
            }
            self.pending_line.push(character);
            if self.pending_line.len() >= MAX_PENDING_LINE_BYTES {
                self.flush_pending_line().await;
            }
        }
    }

    /// Deliver the line being assembled, if there is one.
    async fn flush_pending_line(&mut self) {
        if self.pending_line.is_empty() {
            return;
        }
        let line = ::std::mem::take(&mut self.pending_line);
        self.output.write_line(&line).await;
    }

    /// `word` with its expansions resolved, or `None` when it carries an expansion this
    /// container cannot resolve.
    ///
    /// The pieces come from `brush-parser`'s word parser, so what is quoted, what is a
    /// parameter expansion and what is literal text is the shell's own reading of the word.
    /// A parameter this session holds nothing for expands to nothing, which is what an
    /// unset parameter expands to. A command substitution, an arithmetic expansion or a
    /// tilde prefix is an expansion whose value this container cannot resolve — it would
    /// have to run a command, or resolve a home directory it does not have — so the word
    /// has no expansion here and the command carrying it is not run. Nor does a word that
    /// expands past [`MAX_EXPANSION_BYTES`], or one whose expansion would take the command
    /// past [`MAX_LINE_EXPANSION_BYTES`].
    fn expand_word(&mut self, word: &str) -> Option<String> {
        let options = ::brush_parser::ParserOptions::default();
        let pieces = ::brush_parser::word::parse(word, &options).ok()?;
        let expanded = self.expand_pieces(&pieces, MAX_WORD_DEPTH)?;
        self.expansion_left = self.expansion_left.checked_sub(expanded.len())?;
        Some(expanded)
    }

    /// [`Self::expand_word`]'s walk over the pieces a word parsed into, with `depth` levels
    /// of quoting left: a word nested past [`MAX_WORD_DEPTH`] has no expansion here, so
    /// this walk's own recursion stays in range whatever the word nests.
    fn expand_pieces(
        &self,
        pieces: &[::brush_parser::word::WordPieceWithSource],
        depth: usize,
    ) -> Option<String> {
        let depth = depth.checked_sub(1)?;
        let mut expanded = String::new();
        for piece in pieces {
            let addition = match &piece.piece {
                ::brush_parser::word::WordPiece::Text(text)
                | ::brush_parser::word::WordPiece::SingleQuotedText(text) => text.clone(),
                ::brush_parser::word::WordPiece::DoubleQuotedSequence(inner) => {
                    self.expand_pieces(inner, depth)?
                }
                ::brush_parser::word::WordPiece::ParameterExpansion(parameter) => {
                    self.expand_parameter(parameter)?
                }
                _ => return None,
            };
            if expanded.len().saturating_add(addition.len()) > MAX_EXPANSION_BYTES {
                return None;
            }
            expanded.push_str(&addition);
        }
        Some(expanded)
    }

    /// The value a parameter expansion resolves to in this session.
    ///
    /// The plain form (`$NAME`, `${NAME}`) is the variable's value, empty when the session
    /// holds none. The default-value form is the value or the default the expansion names,
    /// and WHICH values take the default is the form's own: `${NAME:-WORD}` substitutes it
    /// for a parameter that is unset or empty, `${NAME-WORD}` only for one that is unset,
    /// so a variable the container's environment sets to the empty string keeps it under
    /// the second form. Every other form — a substring, a replacement, a length, an
    /// indirection, a positional or special parameter — is one this container does not
    /// resolve.
    fn expand_parameter(&self, parameter: &::brush_parser::word::ParameterExpr) -> Option<String> {
        match parameter {
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
            } => Some(
                self.variables
                    .get(name)
                    .map(String::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            ::brush_parser::word::ParameterExpr::UseDefaultValues {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
                test_type,
                default_value,
            } => {
                let held = self.variables.get(name).map(String::as_str);
                let defaulted = match test_type {
                    // `${NAME:-WORD}`: unset OR null takes the default.
                    ::brush_parser::word::ParameterTestType::UnsetOrNull => {
                        held.is_none_or(str::is_empty)
                    }
                    // `${NAME-WORD}`: only unset takes it.
                    ::brush_parser::word::ParameterTestType::Unset => held.is_none(),
                };
                if defaulted {
                    Some(default_value.clone().unwrap_or_default())
                } else {
                    Some(held.unwrap_or_default().to_string())
                }
            }
            _ => None,
        }
    }
}

/// The name a `trap` signal operand is recorded under: the spelling the program wrote,
/// upper-cased, so `term`, `TERM` and `SIGTERM` are the same signal to this session.
fn signal_key(operand: &str) -> String {
    operand.to_ascii_uppercase()
}
