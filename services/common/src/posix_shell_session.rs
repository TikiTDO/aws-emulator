// SPDX-License-Identifier: CC0-1.0

//! The ONE bounded, session-local POSIX/bash SHELL SESSION every emulated service that has
//! to perform caller-supplied shell text drives.
//!
//! # What this module is
//!
//! Several emulated APIs carry a shell PROGRAM as a caller-supplied member: a build's
//! command lines, a container's `command`, a notebook instance's lifecycle hook. What the
//! client observes is what running that program produced — a status, the lines it wrote, the
//! variables it left behind — so a service that only STORES the text has implemented nothing.
//!
//! The grammar is not read here: a line is tokenized and parsed by `brush-parser`, the
//! POSIX/bash tokenizer and grammar, exactly as this workspace reads caller-supplied SQL
//! with `sqlparser` and caller-supplied YAML with `yaml-rust2`. That parser builds a syntax
//! tree and runs nothing, so the and-or lists, pipelines, assignments and quoting of a
//! program are the shell's own rather than something scanned out of the text. Word expansion
//! is read from the same parser's word pieces.
//!
//! # Why the grammar and not an engine
//!
//! The one maintained shell ENGINE this grammar has — `brush-core`, the executing half of
//! the same project — is deliberately not a dependency, and what follows is not a
//! reimplementation of it. An engine's whole purpose is to run what the shell language can
//! express: it resolves a command name against the host's `PATH` and starts a host PROCESS
//! for it, opens the host files a redirection names, and expands a command substitution by
//! running that command. Those are exactly the primitives this module has none of, and the
//! crate exposes no seam to replace them with an in-memory volume or an in-process
//! dispatcher — its command resolution and its redirections go straight to the host through
//! the standard library — so there is no configuration of it that could be confined here:
//! taking it would hand a caller's program the developer's machine.
//!
//! What this module does instead is narrower than a shell by construction, and it is not an
//! implementation of the shell LANGUAGE: there is no evaluator for the language's constructs
//! here, and none is approximated. It PERFORMS a fixed catalog of session-local utilities
//! ([`BUILT_IN_COMMANDS`]) over the real grammar's tree, and every other command, and every
//! command FORM and expansion it does not perform, fails closed with the status a shell
//! answers for a command it cannot run ([`Statuses::could_not_run`]). There is therefore no
//! language subset to keep in step with a specification: a command either names one of those
//! utilities, or it did not run.
//!
//! # What a session provides
//!
//! It runs IN THIS PROCESS. It has no process to start, no network to reach, no HOST
//! filesystem and no host environment: the utilities it provides are the ones whose whole
//! effect is inside the session itself — its output, its status, its variables and the files
//! it holds itself. The variables it starts with are the ones its caller hands it, because
//! those are the environment the program is given.
//!
//! # The working tree
//!
//! A program's commands write FILES — a build writes the file its artifacts name, a hook
//! writes the configuration it was given — so a session holds a tree of its own: an output
//! redirection (`> path`, `>> path`) collects what the command wrote and keeps it under that
//! path in this session, and `cat` reads a path back out of it. The tree is session-local
//! memory, keyed by the path the program named and bounded like everything else
//! ([`Bounds::files`], [`Bounds::file_bytes`], [`Bounds::tree_bytes`]); no host file is
//! opened, created or read, and every other redirection form (an input, another file
//! descriptor, a duplication, a here-document) is still one this session does not perform.
//!
//! # Bounds
//!
//! Every bound is the emulator's, held in code the program cannot reach, and every one of
//! them is a field of [`Bounds`]: how many commands one run performs (latched — past it a
//! line performs nothing), how long a line may be and how deeply it may nest (checked
//! WITHOUT parsing, which is what keeps the grammar's own recursion in range), how much one
//! word and one line may expand to, how many variables and how many bytes of them the
//! session holds, and — for a caller that KEEPS what the program wrote
//! ([`OutputLimits`]) — how many lines and how many bytes per line it may keep.
//!
//! # The services that drive it
//!
//! * `service-com-amazonaws-codebuild-codebuild-20161006`'s `state::build_shell` performs a
//!   buildspec's command lines for an exit status and keeps no output, which is this module
//!   with [`OutputLimits`] absent;
//! * `service-com-amazonaws-sagemaker-sagemaker`'s `state::notebook_lifecycle_hook` performs
//!   a notebook instance's lifecycle hook and keeps its output, which it publishes to that
//!   hook's CloudWatch Logs stream.
//!
//! One service still carries its OWN copy of this session, written before this module
//! existed, and it is to be MIGRATED onto this one rather than copied again — exactly as the
//! claim-rule and log-group-name copies this crate lists are:
//! `service-com-amazonaws-ecs-amazonec2containerservicev20141113`'s `state::container_shell`
//! performs a container's program as a PROCESS — it loops, waits, installs signal handlers
//! and delivers each line as it is written — so moving it here means ADDING those to this
//! session rather than taking them away, and doing that in a change about neither of the two
//! services above would widen what their programs may run.
//!
//! Nothing new should restate the session — call this module.

use std::collections::BTreeMap;

/// The commands a session provides, as their names.
///
/// Each one's whole effect is inside the session, which is why it can be performed in
/// process: `echo` writes a line of the program's output, `cat` writes a file this session
/// holds, `true`, `false` and `:` answer a status, `exit` ends the session, and
/// `export`/`unset` change its variables. A command that is not one of these needs a
/// program, a host tree or a network a session does not have.
pub const BUILT_IN_COMMANDS: &[&str] = &[
    "echo", "cat", "true", "false", ":", "exit", "export", "unset",
];

/// The commands an assignment written before them persists in the session for: POSIX's
/// special built-in utilities, for which "variable assignments specified with special
/// built-in utilities remain in effect after the built-in completes". For every other command
/// an assignment before it is that command's environment alone.
const PERSISTING_COMMANDS: &[&str] = &[":", "exit", "export", "unset"];

/// The statuses a session answers with, so a service whose API reports a status reports the
/// one a shell answers for its own condition.
#[derive(Debug, Clone, Copy)]
pub struct Statuses {
    /// The status of a command that ran to success.
    pub success: i32,
    /// The status of a command that ran and answered failure, which is what the `false`
    /// utility is specified to do ("exit with a status code indicating failure").
    pub failure: i32,
    /// The status a shell reports for a line it could not parse — what `sh` is specified to
    /// exit with on a syntax error it detects.
    pub syntax_error: i32,
    /// The status a shell reports when "the utility to be executed could not be found",
    /// which is what a session answers for every command it provides no program for, for a
    /// command form it does not perform, and for a line past its bounds.
    pub could_not_run: i32,
}

impl Default for Statuses {
    /// The statuses POSIX gives those four conditions.
    fn default() -> Self {
        Self {
            success: 0,
            failure: 1,
            syntax_error: 2,
            could_not_run: 127,
        }
    }
}

/// How much one session may do. Every field is the EMULATOR'S number, not a modelled one:
/// the programs are caller-supplied text, so what one of them can cost is bounded here.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    /// How many commands one session performs before it is given up on. Latched: past it a
    /// line performs nothing at all.
    pub commands: usize,
    /// The longest line a session reads.
    pub command_bytes: usize,
    /// How many grouping indicators or reserved words one line may carry before the session
    /// gives up on reading it — what keeps the grammar's own recursion in range, since a
    /// parser descends once per nested group.
    pub nesting: usize,
    /// How many bytes one word's expansion may produce.
    pub expansion_bytes: usize,
    /// How many bytes ONE line's expansions may materialise at once, which is their SUM.
    pub line_expansion_bytes: usize,
    /// How many bytes of variables one session may hold at once.
    pub session_bytes: usize,
    /// How many variables one session may hold at once, so neither a few long values nor
    /// very many short ones grow the store without a stated ceiling.
    pub session_variables: usize,
    /// How deeply a word's expansion is walked, applied to the pieces a quoted word nests.
    pub word_depth: usize,
    /// How many files one session's own tree holds at once.
    pub files: usize,
    /// How many bytes ONE of those files may hold, which is also how much of a redirected
    /// command's output is collected for it.
    pub file_bytes: usize,
    /// How many bytes the whole tree may hold at once, so neither a few large files nor very
    /// many small ones grow it without a stated ceiling.
    pub tree_bytes: usize,
}

impl Default for Bounds {
    /// Bounds far above any real program of the kinds these services carry, and far below
    /// what would cost the host anything noticeable.
    fn default() -> Self {
        Self {
            commands: 1_000,
            command_bytes: 16 * 1024,
            nesting: 64,
            expansion_bytes: 64 * 1024,
            line_expansion_bytes: 256 * 1024,
            session_bytes: 1024 * 1024,
            session_variables: 4_096,
            word_depth: 32,
            files: 256,
            file_bytes: 256 * 1024,
            tree_bytes: 4 * 1024 * 1024,
        }
    }
}

/// How much of what a program WROTE a session keeps, for a caller that reports it.
///
/// A caller that reports no output leaves this absent, and then nothing is kept at all: the
/// bytes a command writes are not delivered anywhere, so a session that is not asked for
/// them does not grow with them.
#[derive(Debug, Clone, Copy)]
pub struct OutputLimits {
    /// How many lines one session keeps. Past it a line is not kept at all.
    pub lines: usize,
    /// How many bytes ONE kept line may carry. A longer line is TRUNCATED on a character
    /// boundary rather than dropped, so what the program wrote is still reported.
    pub line_bytes: usize,
}

/// What a session is constructed with: its statuses, its bounds, and whether it keeps what
/// the program wrote.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionConfig {
    /// See [`Statuses`].
    pub statuses: Statuses,
    /// See [`Bounds`].
    pub bounds: Bounds,
    /// See [`OutputLimits`]; `None` keeps no output.
    pub output: Option<OutputLimits>,
}

/// The characters that open a nested group in a line, counted by [`within_parse_bounds`]: a
/// subshell or command substitution, a brace group or expansion, a bracket expression, and
/// the older command-substitution spelling.
const NESTING_INDICATORS: [char; 4] = ['(', '{', '[', '`'];

/// The reserved WORDS that open or close a nested command in a line, counted by
/// [`within_parse_bounds`] beside [`NESTING_INDICATORS`]: the shell's compound commands nest
/// through these rather than through any character, so counting characters alone would leave
/// the grammar's recursion unbounded for a line of `if … then …`. The words that CLOSE one
/// are counted too, which can only make the count higher — an over-count refuses a line and
/// never admits one.
const NESTING_WORDS: [&str; 16] = [
    "if", "then", "elif", "else", "fi", "while", "until", "for", "select", "case", "esac", "in",
    "do", "done", "function", "time",
];

/// The characters a reserved word may be glued to in a line: the shell's own separators and
/// operators. A word is counted when it stands between any of these (or whitespace), so
/// `:;if` carries an `if` exactly as `: ; if` does.
const WORD_SEPARATORS: [char; 10] = [';', '&', '|', '(', ')', '{', '}', '<', '>', '!'];

/// The path a session's tree keys a file by, for the path a program named — `None` when the
/// program named no path at all.
///
/// A relative path is what a program's commands write, and `./name` and `name` are the same
/// file to them, so the one spelling that differs only in that prefix is folded into it.
/// Nothing else about the path is interpreted: it is a key in this session's own tree, never
/// a path on the host.
fn file_path(named: &str) -> Option<String> {
    let trimmed = named.trim();
    let folded = trimmed.strip_prefix("./").unwrap_or(trimmed);
    (!folded.is_empty()).then(|| folded.to_string())
}

/// Whether `line` is inside the bounds a parse is given: its size, and how many nested
/// commands it opens — through a grouping character or a reserved word.
///
/// Checked WITHOUT parsing, which is the point: the parser is what the nesting bound
/// protects.
fn within_parse_bounds(line: &str, bounds: &Bounds) -> bool {
    if line.len() > bounds.command_bytes {
        return false;
    }
    let grouped = line
        .chars()
        .filter(|character| NESTING_INDICATORS.contains(character))
        .count();
    let worded = line
        .split(|character: char| character.is_whitespace() || WORD_SEPARATORS.contains(&character))
        .filter(|word| NESTING_WORDS.contains(word))
        .count();
    grouped.saturating_add(worded) <= bounds.nesting
}

/// The file descriptor a session performs a redirection of: standard output, the one a
/// command's own writes go to. A redirection of any other descriptor is a form this session
/// does not perform.
const STANDARD_OUTPUT: i32 = 1;

/// What the command a redirection is in force for wrote, collected for the file it names.
#[derive(Debug, Default)]
struct Capture {
    /// The lines the command wrote, in the order it wrote them.
    lines: Vec<String>,
    /// How many bytes those lines come to, kept as they are collected.
    bytes: usize,
    /// Set once the command wrote more than one file may hold ([`Bounds::file_bytes`]):
    /// the write did not fit, so the command did not perform it.
    overflowed: bool,
}

/// The session one caller-supplied program runs in: the variables its commands see and
/// change, what its `echo` commands wrote (when the caller keeps output), the files its
/// redirections left behind, and how much running is left in its budget.
#[derive(Debug)]
pub struct ShellSession {
    config: SessionConfig,
    /// The variables this session holds, which is what a word's parameter expansion resolves
    /// against and what a caller reads an exported variable from.
    variables: BTreeMap<String, String>,
    /// How many bytes those variables' names and values come to, kept as they are written
    /// rather than recomputed, so the session-bytes check costs one addition however many
    /// variables the session holds.
    held_bytes: usize,
    /// How many more bytes the line being run may expand to, reset for each line.
    expansion_left: usize,
    /// How many more commands this session may perform.
    commands_left: usize,
    /// The status `exit` ended this session with, once a command asked for one: every later
    /// line is then not performed at all, the way a shell stops at `exit`.
    exit_status: Option<i32>,
    /// What the program wrote, one entry per performed `echo`, kept only when the
    /// configuration asks for it.
    output: Vec<String>,
    /// The files this session's own tree holds, keyed by the path a redirection named.
    files: BTreeMap<String, String>,
    /// How many bytes that tree comes to, kept as it is written.
    tree_bytes: usize,
    /// Set while a redirection is in force for the command being performed: what it writes
    /// is collected here for the file rather than kept as the program's own output.
    captured: Option<Capture>,
}

impl ShellSession {
    /// A session holding `variables`, with the whole command budget `config` allows left.
    #[must_use]
    pub fn new(variables: BTreeMap<String, String>, config: SessionConfig) -> Self {
        let held_bytes = variables
            .iter()
            .map(|(name, value)| name.len().saturating_add(value.len()))
            .sum();
        Self {
            variables,
            held_bytes,
            expansion_left: config.bounds.line_expansion_bytes,
            commands_left: config.bounds.commands,
            exit_status: None,
            output: Vec::new(),
            files: BTreeMap::new(),
            tree_bytes: 0,
            captured: None,
            config,
        }
    }

    /// The value this session holds for `name`, or `None` when it holds none.
    #[must_use]
    pub fn variable(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(String::as_str)
    }

    /// The status `exit` ended this session with, or `None` while nothing has.
    #[must_use]
    pub fn exit_status(&self) -> Option<i32> {
        self.exit_status
    }

    /// The lines this session's `echo` commands wrote, in the order they wrote them. Empty
    /// for a session whose configuration keeps no output.
    #[must_use]
    pub fn output(&self) -> &[String] {
        &self.output
    }

    /// [`Self::output`], taking the lines rather than borrowing them.
    #[must_use]
    pub fn into_output(self) -> Vec<String> {
        self.output
    }


    /// Perform one LINE of the program and answer the status the shell answered for it.
    ///
    /// A line that does not parse is the shell's own syntax error
    /// ([`Statuses::syntax_error`]); a line outside the bounds this parse is given
    /// ([`within_parse_bounds`]), or one reached after the command budget is spent, is a
    /// line this session did not perform at all, which is reported as a command it could
    /// not run ([`Statuses::could_not_run`]) rather than as one that succeeded.
    pub fn run(&mut self, line: &str) -> i32 {
        if !within_parse_bounds(line, &self.config.bounds) || self.commands_left == 0 {
            return self.config.statuses.could_not_run;
        }
        // Each line gets the whole expansion budget: the words of one line are live at the
        // same instant, and the words of the line before it are not.
        self.expansion_left = self.config.bounds.line_expansion_bytes;
        let Ok(tokens) = ::brush_parser::tokenize_str(line) else {
            return self.config.statuses.syntax_error;
        };
        let options = ::brush_parser::ParserOptions::default();
        let Ok(program) = ::brush_parser::parse_tokens(&tokens, &options) else {
            return self.config.statuses.syntax_error;
        };
        let mut status = self.config.statuses.success;
        for complete_command in &program.complete_commands {
            for item in &complete_command.0 {
                status = self.run_and_or_list(&item.0);
                if self.exit_status.is_some() {
                    return status;
                }
            }
        }
        status
    }

    /// Perform an and-or list: its first pipeline, then each later pipeline only when the
    /// operator before it says so — `&&` runs the next on a zero status, `||` on a non-zero
    /// one — and answer the status of the last pipeline that ran.
    fn run_and_or_list(&mut self, list: &::brush_parser::ast::AndOrList) -> i32 {
        let mut status = self.run_pipeline(&list.first);
        for next in &list.additional {
            if self.exit_status.is_some() {
                break;
            }
            match next {
                ::brush_parser::ast::AndOr::And(pipeline) => {
                    if status == self.config.statuses.success {
                        status = self.run_pipeline(pipeline);
                    }
                }
                ::brush_parser::ast::AndOr::Or(pipeline) => {
                    if status != self.config.statuses.success {
                        status = self.run_pipeline(pipeline);
                    }
                }
            }
        }
        status
    }

    /// Perform a pipeline and answer its status: "the exit status of a pipeline shall be the
    /// exit status of the last command", negated when the pipeline is written with `!`.
    ///
    /// Every command of the pipeline is performed, in order, because each answers its own
    /// status; this session passes no bytes from one command to the next, so a pipeline's
    /// commands each read nothing.
    fn run_pipeline(&mut self, pipeline: &::brush_parser::ast::Pipeline) -> i32 {
        let mut status = self.config.statuses.success;
        for command in &pipeline.seq {
            status = self.run_command(command);
            if self.exit_status.is_some() {
                break;
            }
        }
        if pipeline.bang {
            status = if status == self.config.statuses.success {
                self.config.statuses.failure
            } else {
                self.config.statuses.success
            };
        }
        status
    }

    /// Perform one command of a pipeline.
    ///
    /// A SIMPLE command is the form this session performs. Every other form a shell grammar
    /// admits — a compound command, a function definition, an extended test — is one this
    /// session does not perform, and a command that did not run answers as one that could
    /// not be run rather than as one that succeeded.
    fn run_command(&mut self, command: &::brush_parser::ast::Command) -> i32 {
        match command {
            ::brush_parser::ast::Command::Simple(simple) => self.run_simple_command(simple),
            _ => self.config.statuses.could_not_run,
        }
    }

    /// Perform a simple command: its variable assignments, then the command its first word
    /// names with the words after it as its arguments, with what it writes going to the file
    /// an output redirection names.
    ///
    /// A command with no word at all is the assignment form — "the variable assignments
    /// shall affect the current execution environment" — which is how a program sets a
    /// variable a caller later reads. An assignment written BEFORE a command word affects
    /// that command's environment alone, and persists in the session only for the special
    /// built-in utilities POSIX keeps it for ([`PERSISTING_COMMANDS`]); since every word is
    /// expanded before the assignments are applied, a command that does not keep them sees
    /// the value either way. An output redirection of standard output collects what the
    /// command wrote into this session's own tree ([`Self::redirection_of`]); any other
    /// prefix or suffix item is a redirection form or a substitution this session does not
    /// perform, and the command is then not run.
    ///
    /// The file a redirection names is written whatever the command answered, the way a
    /// shell opens it before it resolves the command at all: a command this session has no
    /// utility for leaves the file it redirected to behind, empty.
    fn run_simple_command(&mut self, simple: &::brush_parser::ast::SimpleCommand) -> i32 {
        self.commands_left = self.commands_left.saturating_sub(1);
        let mut assignments: Vec<(String, String)> = Vec::new();
        let mut words: Vec<String> = Vec::new();
        let mut redirected: Option<(String, bool)> = None;
        let prefix = simple.prefix.as_ref().map(|prefix| &prefix.0);
        let suffix = simple.suffix.as_ref().map(|suffix| &suffix.0);
        for item in prefix.into_iter().chain(suffix).flatten() {
            match item {
                ::brush_parser::ast::CommandPrefixOrSuffixItem::Word(word) => {
                    let Some(expanded) = self.expand_word(&word.value) else {
                        return self.config.statuses.could_not_run;
                    };
                    words.push(expanded);
                }
                ::brush_parser::ast::CommandPrefixOrSuffixItem::AssignmentWord(assignment, _) => {
                    let Some(pair) = self.assignment_of(assignment) else {
                        return self.config.statuses.could_not_run;
                    };
                    assignments.push(pair);
                }
                ::brush_parser::ast::CommandPrefixOrSuffixItem::IoRedirect(redirect) => {
                    let Some(opened) = self.redirection_of(redirect) else {
                        return self.config.statuses.could_not_run;
                    };
                    redirected = Some(opened);
                }
                _ => return self.config.statuses.could_not_run,
            }
        }
        let name = match simple.word_or_name.as_ref() {
            Some(word) => match self.expand_word(&word.value) {
                Some(expanded) => Some(expanded),
                None => return self.config.statuses.could_not_run,
            },
            None => None,
        };
        let persists = name
            .as_deref()
            .is_none_or(|name| PERSISTING_COMMANDS.contains(&name));
        if persists {
            for (name, value) in assignments {
                if !self.set_variable(name, value) {
                    return self.config.statuses.could_not_run;
                }
            }
        }
        if redirected.is_some() {
            self.captured = Some(Capture::default());
        }
        let status = match name {
            Some(name) => self.run_built_in(&name, &words),
            // Nothing but assignments, or a redirection alone: they have been applied, which
            // is the whole command.
            None => self.config.statuses.success,
        };
        let captured = self.captured.take();
        match redirected {
            Some((path, append)) => {
                if self.write_file(&path, append, captured.unwrap_or_default()) {
                    status
                } else {
                    self.config.statuses.could_not_run
                }
            }
            None => status,
        }
    }

    /// The file one redirection of a simple command writes, as `(path, append)` — `None`
    /// when it is a form this session does not perform.
    ///
    /// The two forms it does are the output redirections: `> path` writes the file from the
    /// start and `>> path` adds to what it holds ("the file shall be opened for
    /// appending"), each of standard output, which is where a command's own writes go.
    /// Every other form names a descriptor, a file or a document this session has nothing
    /// to connect: an input (there is no program to feed), another descriptor, a duplication,
    /// a here-document or a process substitution.
    fn redirection_of(&mut self, redirect: &::brush_parser::ast::IoRedirect) -> Option<(String, bool)> {
        let ::brush_parser::ast::IoRedirect::File(descriptor, kind, target) = redirect else {
            return None;
        };
        if descriptor.is_some_and(|descriptor| descriptor != STANDARD_OUTPUT) {
            return None;
        }
        let append = match kind {
            ::brush_parser::ast::IoFileRedirectKind::Write => false,
            ::brush_parser::ast::IoFileRedirectKind::Append => true,
            _ => return None,
        };
        let ::brush_parser::ast::IoFileRedirectTarget::Filename(word) = target else {
            return None;
        };
        let named = self.expand_word(&word.value)?;
        Some((file_path(&named)?, append))
    }

    /// Keep `lines` as the file `path`, from the start or added to what that file already
    /// holds.
    ///
    /// Each line is written with the newline that ended it, which is what a command's output
    /// carries. `false` is a write this session did not perform — the command wrote more
    /// than one file may hold, or the tree is already holding [`Bounds::files`] files or
    /// [`Bounds::tree_bytes`] bytes — which its caller reports as a command that could not
    /// be run rather than as one that wrote the file.
    fn write_file(&mut self, path: &str, append: bool, captured: Capture) -> bool {
        if captured.overflowed {
            return false;
        }
        let mut whole = if append {
            self.files.get(path).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        for line in &captured.lines {
            whole.push_str(line);
            whole.push('\n');
        }
        if whole.len() > self.config.bounds.file_bytes {
            return false;
        }
        let displaced = self
            .files
            .get(path)
            .map(|held| path.len().saturating_add(held.len()));
        if displaced.is_none() && self.files.len() >= self.config.bounds.files {
            return false;
        }
        let would_hold = self
            .tree_bytes
            .saturating_sub(displaced.unwrap_or(0))
            .saturating_add(path.len())
            .saturating_add(whole.len());
        if would_hold > self.config.bounds.tree_bytes {
            return false;
        }
        self.tree_bytes = would_hold;
        self.files.insert(path.to_string(), whole);
        true
    }

    /// One assignment of a simple command as the name and value it sets, or `None` when it
    /// is a form this session does not perform — an array assignment, an appending
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
    /// [`Bounds::session_bytes`] or [`Bounds::session_variables`] — `false` is then a
    /// command this session did not perform, which is what its caller reports.
    fn set_variable(&mut self, name: String, value: String) -> bool {
        let displaced = self
            .variables
            .get(&name)
            .map(|held| name.len().saturating_add(held.len()));
        if displaced.is_none() && self.variables.len() >= self.config.bounds.session_variables {
            return false;
        }
        let would_hold = self
            .held_bytes
            .saturating_sub(displaced.unwrap_or(0))
            .saturating_add(name.len())
            .saturating_add(value.len());
        if would_hold > self.config.bounds.session_bytes {
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

    /// Write one line of the program's output, which is what `echo` does: the words it was
    /// given, separated by one space, as the shell's own `echo` writes them.
    ///
    /// While a redirection is in force the line is collected for the file it names, so it is
    /// not kept as the program's own output; a line that would take the write past
    /// [`Bounds::file_bytes`] is not collected and the write does not happen at all.
    ///
    /// Otherwise nothing is kept for a session whose configuration asks for no output. A
    /// line past [`OutputLimits::line_bytes`] is truncated on a character boundary, and a
    /// line past [`OutputLimits::lines`] is not kept at all.
    fn write_line(&mut self, words: &[String]) {
        let mut line = words.join(" ");
        let file_bytes = self.config.bounds.file_bytes;
        if let Some(captured) = self.captured.as_mut() {
            let written = captured
                .bytes
                .saturating_add(line.len())
                .saturating_add(1);
            if written > file_bytes {
                captured.overflowed = true;
                return;
            }
            captured.bytes = written;
            captured.lines.push(line);
            return;
        }
        let Some(limits) = self.config.output else {
            return;
        };
        if self.output.len() >= limits.lines {
            return;
        }
        if line.len() > limits.line_bytes {
            let mut cut = limits.line_bytes;
            while cut > 0 && !line.is_char_boundary(cut) {
                cut -= 1;
            }
            line.truncate(cut);
        }
        self.output.push(line);
    }

    /// Perform the command `name` with `arguments`, when it is one this session provides
    /// ([`BUILT_IN_COMMANDS`]).
    ///
    /// `echo` writes a line of the program's output. `cat` writes the lines of each file
    /// this session's tree holds for the paths it was given. `true` and `:` answer success
    /// and `false` answers failure, which is the whole of what those utilities do. `exit`
    /// ends the session with the status its argument names, defaulting to success, and a
    /// status outside 0..=255 is taken modulo 256 the way a shell takes an exit status.
    /// `export` sets the variables its `NAME=VALUE` arguments name and `unset` removes them.
    fn run_built_in(&mut self, name: &str, arguments: &[String]) -> i32 {
        // The catalog is what this session provides: a command outside it needs a program, a
        // host tree or a network this session does not have.
        if !BUILT_IN_COMMANDS.contains(&name) {
            return self.config.statuses.could_not_run;
        }
        match name {
            "echo" => {
                self.write_line(arguments);
                self.config.statuses.success
            }
            "cat" => self.run_cat(arguments),
            "true" | ":" => self.config.statuses.success,
            "false" => self.config.statuses.failure,
            "exit" => {
                let status = match arguments.first() {
                    Some(argument) => match argument.parse::<i64>() {
                        Ok(status) => {
                            i32::try_from(status.rem_euclid(256)).unwrap_or(self.config.statuses.failure)
                        }
                        // A status a shell cannot read is its own usage failure, and the
                        // session still ends.
                        Err(_) => self.config.statuses.failure,
                    },
                    None => self.config.statuses.success,
                };
                self.exit_status = Some(status);
                status
            }
            "export" => {
                for argument in arguments {
                    // `export NAME=VALUE` sets it; `export NAME` marks a variable this
                    // session already holds, and marking one it does not hold sets nothing.
                    if let Some((name, value)) = argument.split_once('=') {
                        if !self.set_variable(name.to_string(), value.to_string()) {
                            return self.config.statuses.could_not_run;
                        }
                    }
                }
                self.config.statuses.success
            }
            "unset" => {
                for argument in arguments {
                    self.unset_variable(argument);
                }
                self.config.statuses.success
            }
            _ => self.config.statuses.could_not_run,
        }
    }

    /// Perform `cat`: write the lines of each file this session's tree holds for the paths it
    /// was given.
    ///
    /// "cat … shall exit with a value >0 if an error occurred", and a path this tree holds no
    /// file for is one this session could not read, so it answers failure while still writing
    /// the files it does hold. `cat` with no operand reads standard input, which a session
    /// has none of, so that form is one it does not perform.
    fn run_cat(&mut self, arguments: &[String]) -> i32 {
        if arguments.is_empty() {
            return self.config.statuses.could_not_run;
        }
        let mut status = self.config.statuses.success;
        for argument in arguments {
            let held = file_path(argument).and_then(|path| self.files.get(&path).cloned());
            match held {
                Some(content) => {
                    for line in content.lines() {
                        self.write_line(&[line.to_string()]);
                    }
                }
                None => status = self.config.statuses.failure,
            }
        }
        status
    }

    /// `word` with its expansions resolved, or `None` when it carries an expansion this
    /// session cannot resolve.
    ///
    /// The pieces come from `brush-parser`'s word parser, so what is quoted, what is a
    /// parameter expansion and what is literal text is the shell's own reading of the word.
    /// A parameter this session holds nothing for expands to nothing, which is what an unset
    /// parameter expands to. A command substitution, an arithmetic expansion or a tilde
    /// prefix is an expansion whose value this session cannot resolve — it would have to run
    /// a command, or resolve a home directory it does not have — so the word has no
    /// expansion here and the command carrying it is not run. Nor does a word that expands
    /// past [`Bounds::expansion_bytes`], or one whose expansion would take the line past
    /// [`Bounds::line_expansion_bytes`].
    fn expand_word(&mut self, word: &str) -> Option<String> {
        let options = ::brush_parser::ParserOptions::default();
        let pieces = ::brush_parser::word::parse(word, &options).ok()?;
        let expanded = self.expand_pieces(&pieces, self.config.bounds.word_depth)?;
        self.expansion_left = self.expansion_left.checked_sub(expanded.len())?;
        Some(expanded)
    }

    /// [`Self::expand_word`]'s walk over the pieces a word parsed into, with `depth` levels
    /// of quoting left: a word nested past [`Bounds::word_depth`] has no expansion here, so
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
            if expanded.len().saturating_add(addition.len()) > self.config.bounds.expansion_bytes {
                return None;
            }
            expanded.push_str(&addition);
        }
        Some(expanded)
    }

    /// The value a parameter expansion resolves to in this session.
    ///
    /// The plain form (`$NAME`, `${NAME}`) is the variable's value, empty when the session
    /// holds none, and the default-value form (`${NAME:-WORD}`) is that value or the default
    /// the expansion names. Every other form — a substring, a replacement, a length, an
    /// indirection, a positional or special parameter — is one this session does not
    /// resolve.
    fn expand_parameter(&self, parameter: &::brush_parser::word::ParameterExpr) -> Option<String> {
        match parameter {
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
            } => Some(self.variable(name).unwrap_or_default().to_string()),
            ::brush_parser::word::ParameterExpr::UseDefaultValues {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
                default_value,
                ..
            } => {
                let held = self.variable(name).unwrap_or_default();
                if held.is_empty() {
                    Some(default_value.clone().unwrap_or_default())
                } else {
                    Some(held.to_string())
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Bounds, OutputLimits, SessionConfig, ShellSession, Statuses, BUILT_IN_COMMANDS,
    };
    use std::collections::BTreeMap;

    /// A session that keeps its output, with the default statuses and bounds.
    fn session() -> ShellSession {
        ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                output: Some(OutputLimits {
                    lines: 8,
                    line_bytes: 64,
                }),
                ..SessionConfig::default()
            },
        )
    }

    /// Perform every line of `program` in one session and answer it.
    fn run(program: &str) -> ShellSession {
        let mut held = session();
        for line in program.lines() {
            held.run(line);
            if held.exit_status().is_some() {
                break;
            }
        }
        held
    }

    /// RULE: only the catalog's utilities are performed; every other command and every form
    /// this session does not perform answers "could not run" and writes nothing.
    #[test]
    fn only_the_catalog_is_performed() {
        let statuses = Statuses::default();
        // (case, the line, the status it answers, the lines it wrote)
        let rows: [(&str, &str, i32, &[&str]); 9] = [
            ("echo", "echo one two", statuses.success, &["one two"]),
            ("true", "true", statuses.success, &[]),
            (":", ":", statuses.success, &[]),
            ("false", "false", statuses.failure, &[]),
            (
                "a host program",
                "curl https://example.invalid",
                statuses.could_not_run,
                &[],
            ),
            (
                "an output redirection writes the session's own file",
                "echo out > kept.txt",
                statuses.success,
                &[],
            ),
            (
                "an input redirection",
                "cat < kept.txt",
                statuses.could_not_run,
                &[],
            ),
            (
                "a command substitution",
                "echo $(whoami)",
                statuses.could_not_run,
                &[],
            ),
            (
                "a compound command",
                "if true; then echo inside; fi",
                statuses.could_not_run,
                &[],
            ),
        ];
        for (case, line, expected_status, expected_output) in rows {
            let mut held = session();
            let status = held.run(line);
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected status {expected_status}, got {status}"
            );
            assert_eq!(
                held.output(),
                expected_output,
                "case {case:?}: expected {expected_output:?}, got {:?}",
                held.output()
            );
        }
        assert_eq!(
            BUILT_IN_COMMANDS.len(),
            8,
            "case the catalog: expected the eight session-local utilities, got {BUILT_IN_COMMANDS:?}"
        );
    }

    /// RULE: a program's commands write FILES in the session's own tree — an output
    /// redirection keeps what the command wrote under the path it named (`>` from the start,
    /// `>>` added to what the file holds), `cat` reads one back, and a path the tree holds no
    /// file for is one `cat` could not read.
    ///
    /// The rows differ along what the rule generalises over: which redirection operator the
    /// program used, how many commands wrote the file, the path's spelling, whether the file
    /// was ever written, and which command the redirection applies to.
    #[test]
    fn a_program_s_commands_write_files_in_the_session_s_tree() {
        let statuses = Statuses::default();
        // (case, the program, the status its last line answered, the lines it wrote)
        let rows: [(&str, &str, i32, &[&str]); 8] = [
            (
                "a redirected write read back",
                "echo first > out.txt\ncat out.txt",
                statuses.success,
                &["first"],
            ),
            (
                "a second write from the start replaces the file",
                "echo first > out.txt\necho second > out.txt\ncat out.txt",
                statuses.success,
                &["second"],
            ),
            (
                "an appending write adds to it",
                "echo first > out.txt\necho second >> out.txt\ncat out.txt",
                statuses.success,
                &["first", "second"],
            ),
            (
                "an appending write to a file nothing wrote yet",
                "echo only >> out.txt\ncat out.txt",
                statuses.success,
                &["only"],
            ),
            (
                "the same file named with and without a leading ./",
                "echo first > ./out.txt\ncat out.txt",
                statuses.success,
                &["first"],
            ),
            (
                "a path nothing wrote",
                "cat missing.txt",
                statuses.failure,
                &[],
            ),
            (
                "an expansion names the path and the content",
                "NAME=out.txt\nWHAT=written\necho $WHAT > $NAME\ncat out.txt",
                statuses.success,
                &["written"],
            ),
            (
                "a redirected command this session has no utility for leaves the file empty",
                "mvn package > out.txt\ncat out.txt",
                statuses.success,
                &[],
            ),
        ];
        for (case, program, expected_status, expected_output) in rows {
            let mut held = session();
            let mut status = statuses.success;
            for line in program.lines() {
                status = held.run(line);
            }
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected status {expected_status}, got {status}"
            );
            assert_eq!(
                held.output(),
                expected_output,
                "case {case:?}: expected {expected_output:?}, got {:?}",
                held.output()
            );
        }
    }

    /// RULE: the tree is bounded like everything else — a write past the per-file ceiling,
    /// one past the whole tree's, and a file past the file count are each a write this
    /// session did not perform, and the file it would have written is not there.
    #[test]
    fn the_tree_s_bounds_stop_the_write() {
        let statuses = Statuses::default();

        // One file may hold no more than `file_bytes`.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    file_bytes: 8,
                    ..Bounds::default()
                },
                output: Some(OutputLimits {
                    lines: 8,
                    line_bytes: 64,
                }),
                ..SessionConfig::default()
            },
        );
        let status = held.run("echo 0123456789 > out.txt");
        assert_eq!(
            status, statuses.could_not_run,
            "case the per-file ceiling: expected {}, got {status}",
            statuses.could_not_run
        );
        assert_eq!(
            held.run("cat out.txt"),
            statuses.failure,
            "case the per-file ceiling: the file must not be there, got {:?}",
            held.output()
        );

        // The tree holds no more than `files` files.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    files: 1,
                    ..Bounds::default()
                },
                ..SessionConfig::default()
            },
        );
        assert_eq!(
            held.run("echo one > first.txt"),
            statuses.success,
            "case the file count: the first file is written"
        );
        assert_eq!(
            held.run("echo two > second.txt"),
            statuses.could_not_run,
            "case the file count: a file past the count is not written"
        );
        assert_eq!(
            held.run("echo again > first.txt"),
            statuses.success,
            "case the file count: a file the tree already holds is still written"
        );

        // The whole tree holds no more than `tree_bytes`.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    tree_bytes: 24,
                    ..Bounds::default()
                },
                ..SessionConfig::default()
            },
        );
        assert_eq!(
            held.run("echo onetwothree > first.txt"),
            statuses.success,
            "case the tree ceiling: the first file fits"
        );
        assert_eq!(
            held.run("echo onetwothree > second.txt"),
            statuses.could_not_run,
            "case the tree ceiling: the write past it is not performed"
        );
    }

    /// RULE: a variable the program sets is what its parameter expansions resolve against,
    /// an unset parameter expands to nothing, and `unset` removes one.
    #[test]
    fn variables_are_the_session_s_own() {
        // (case, the program, the line it wrote)
        let rows: [(&str, &str, &str); 5] = [
            ("an assignment and an expansion", "A=one\necho $A", "one"),
            ("export sets a variable", "export B=two\necho ${B}", "two"),
            ("an unset parameter", "echo [${ABSENT}]", "[]"),
            (
                "a default value",
                "echo ${ABSENT:-fallback}",
                "fallback",
            ),
            ("unset removes it", "C=three\nunset C\necho [$C]", "[]"),
        ];
        for (case, program, expected) in rows {
            let held = run(program);
            let written = held.output().last().cloned().unwrap_or_default();
            assert_eq!(
                written, expected,
                "case {case:?}: expected {expected:?}, got {written:?}"
            );
        }
    }

    /// RULE: `exit` ends the session with the status it names, and nothing after it is
    /// performed.
    #[test]
    fn exit_ends_the_session() {
        for (case, program, expected_status, expected_lines) in [
            ("exit with no status", "echo one\nexit\necho two", 0, 1),
            ("exit 3", "echo one\nexit 3\necho two", 3, 1),
            ("exit past 255 wraps", "exit 300", 44, 0),
        ] {
            let held = run(program);
            assert_eq!(
                held.exit_status(),
                Some(expected_status),
                "case {case:?}: expected the session to end with {expected_status}, got {:?}",
                held.exit_status()
            );
            assert_eq!(
                held.output().len(),
                expected_lines,
                "case {case:?}: expected {expected_lines} line(s), got {:?}",
                held.output()
            );
        }
    }

    /// RULE: every bound is the session's own — the command budget, the line size, the
    /// nesting count, the variable count and the output ceilings each stop the work.
    #[test]
    fn every_bound_stops_the_work() {
        let statuses = Statuses::default();

        // The command budget is latched: past it a line performs nothing.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    commands: 2,
                    ..Bounds::default()
                },
                output: Some(OutputLimits {
                    lines: 8,
                    line_bytes: 64,
                }),
                ..SessionConfig::default()
            },
        );
        for _ in 0..4 {
            held.run("echo line");
        }
        assert_eq!(
            held.output().len(),
            2,
            "case the command budget: expected 2 performed commands, got {:?}",
            held.output()
        );

        // A line past the size bound, and one past the nesting bound, are not read at all.
        let mut held = session();
        let long = format!("echo {}", "x".repeat(Bounds::default().command_bytes));
        assert_eq!(
            held.run(&long),
            statuses.could_not_run,
            "case an oversized line: expected it to perform nothing"
        );
        let nested = format!("echo {}", "(".repeat(Bounds::default().nesting + 1));
        assert_eq!(
            held.run(&nested),
            statuses.could_not_run,
            "case a deeply nested line: expected it to perform nothing"
        );
        assert!(
            held.output().is_empty(),
            "case the parse bounds: expected no output, got {:?}",
            held.output()
        );

        // The variable count is bounded: a further assignment performs nothing.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    session_variables: 1,
                    ..Bounds::default()
                },
                ..SessionConfig::default()
            },
        );
        assert_eq!(
            held.run("A=one"),
            statuses.success,
            "case the first variable: expected it to be set"
        );
        assert_eq!(
            held.run("B=two"),
            statuses.could_not_run,
            "case the variable ceiling: expected the assignment to perform nothing"
        );

        // The output ceilings: at most `lines` entries, each at most `line_bytes` bytes.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                output: Some(OutputLimits {
                    lines: 2,
                    line_bytes: 4,
                }),
                ..SessionConfig::default()
            },
        );
        for _ in 0..5 {
            held.run("echo abcdefgh");
        }
        assert_eq!(
            held.output(),
            ["abcd", "abcd"],
            "case the output ceilings: expected two truncated lines, got {:?}",
            held.output()
        );

        // A session that keeps no output keeps none, whatever the program writes.
        let mut held = ShellSession::new(BTreeMap::new(), SessionConfig::default());
        held.run("echo nothing-is-kept");
        assert!(
            held.output().is_empty(),
            "case no output limits: expected nothing kept, got {:?}",
            held.output()
        );
    }
}
