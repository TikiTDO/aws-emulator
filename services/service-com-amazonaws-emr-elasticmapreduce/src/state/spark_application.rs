// SPDX-License-Identifier: CC0-1.0

//! WHAT a step submits, when what it submits is a Spark application.
//!
//! One concern of this service's state, beside [`super::step_runner`]: that module reads the
//! SQL a `command-runner.jar` command carries, and this one reads the other class of command
//! a step of a Spark cluster carries — a `spark-submit` of an application.
//!
//! "command-runner.jar … lets you run commands on your cluster" (Amazon EMR
//! documentation), so a step's arguments ARE the command line, and `spark-submit` is the
//! command a Spark application is submitted with: `spark-submit [options] <application>
//! [application arguments]`, where the first positional word is the application and the
//! words after it are its own (the `spark-submit --help` grammar). What this module answers
//! is which application a step submits and with which arguments, read off that command line
//! by walking that grammar — never by matching the command's text.
//!
//! Only ONE class of application can be run here, and it is the class the emulator holds an
//! engine for: a PYTHON application whose file is an object in Amazon S3. Its code is
//! caller-supplied Python, which this workspace runs in the contained interpreter it already
//! embeds ([`super::spark_step_run`]), and its file is an object the emulator's own object
//! store holds, so the bytes that run are the bytes a client uploaded. A step submitting
//! anything else — a jar, a file on a node's own disk, a command that is not a
//! `spark-submit` — submits no application this service can account for, and ends the way
//! every unaccounted-for step does ([`super::STEP_NOT_EXECUTED_REASON`]).

use super::*;

/// The application the release catalog publishes the version of for a Spark cluster, as it
/// names it ([`super::release_application_version`]).
pub const SPARK_APPLICATION_NAME: &str = "Spark";

/// The command a Spark application is submitted with, as a step's command line names it.
pub const SPARK_SUBMIT_COMMAND: &str = "spark-submit";

/// The suffix of a Python application's file: `spark-submit` takes "a .py file" as the
/// application for a Python application, as against a jar for a JVM one.
pub const PYTHON_APPLICATION_SUFFIX: &str = ".py";

/// The URI schemes that name an object of Amazon S3, as a Spark application's path spells
/// one: the EMR filesystem's own scheme and the Hadoop S3A/S3N ones.
pub const S3_URI_SCHEMES: &[&str] = &["s3://", "s3a://", "s3n://"];

/// How many words of a command line this reading walks. CHOSEN: a bound on what one step's
/// command can cost to read, far above any real `spark-submit` line.
const MAX_COMMAND_WORDS: usize = 512;

/// The `spark-submit` options that take a VALUE in the word after them, so the word after
/// one of these is never the application.
///
/// SOURCE: `spark-submit --help`'s own option list. An option written with `=`
/// (`--conf k=v`) carries its value in the same word and needs no entry; a boolean flag
/// (`--verbose`, `--supervise`) takes no value and is skipped as the option word it is.
const VALUE_OPTIONS: &[&str] = &[
    "--master",
    "--deploy-mode",
    "--class",
    "--name",
    "--jars",
    "--packages",
    "--exclude-packages",
    "--repositories",
    "--py-files",
    "--files",
    "--archives",
    "--conf",
    "-c",
    "--properties-file",
    "--driver-memory",
    "--driver-java-options",
    "--driver-library-path",
    "--driver-class-path",
    "--driver-cores",
    "--executor-memory",
    "--executor-cores",
    "--total-executor-cores",
    "--num-executors",
    "--proxy-user",
    "--queue",
    "--principal",
    "--keytab",
];

/// An object of Amazon S3, as a path names one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Object {
    pub bucket: String,
    pub key: String,
}

/// The Python application a step submits: the object its code is in, and the arguments the
/// command line passes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparkApplication {
    /// The application file, as the command named it — what its own `sys.argv[0]` reports.
    pub path: String,
    /// The object that file is.
    pub script: S3Object,
    /// The application's own arguments, in order.
    pub arguments: Vec<String>,
}

/// The object `path` names, or `None` when it names no Amazon S3 object.
#[must_use]
pub fn s3_object(path: &str) -> Option<S3Object> {
    let rest = S3_URI_SCHEMES
        .iter()
        .find_map(|scheme| path.strip_prefix(scheme))?;
    let (bucket, key) = rest.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then(|| S3Object {
        bucket: bucket.to_string(),
        key: key.to_string(),
    })
}

/// The words of the `spark-submit` command a step's command line is, or `None` when the step
/// submits no `spark-submit` at all.
///
/// A step's own arguments are the command line ([`SPARK_SUBMIT_COMMAND`] as their first
/// word, however the submitter spelled its path), and a command line may also arrive as ONE
/// argument holding a whole shell command (the `bash -c '…'` form), which is lexed with the
/// POSIX shell's word rules rather than split by hand — the same reading
/// [`super::read_step_work`] gives a command that carries SQL.
#[must_use]
pub fn spark_submit_words(jar: &str, args: &[String]) -> Option<Vec<String>> {
    if !names_command_runner(jar) {
        return None;
    }
    if let Some(words) = submitted_words(args) {
        return Some(words);
    }
    args.iter()
        .take(MAX_COMMAND_WORDS)
        .filter_map(|argument| ::shell_words::split(argument).ok())
        .filter(|lexed| lexed.len() > 1)
        .find_map(|lexed| submitted_words(&lexed))
}

/// `words` when their first word names [`SPARK_SUBMIT_COMMAND`].
fn submitted_words(words: &[String]) -> Option<Vec<String>> {
    let command = words.first()?;
    let program = command.rsplit('/').next().unwrap_or(command);
    (program == SPARK_SUBMIT_COMMAND)
        .then(|| words.iter().take(MAX_COMMAND_WORDS).cloned().collect())
}

/// The Python application a step's command submits, or `None` when it submits none this
/// service can run.
///
/// The application is the FIRST positional word of the `spark-submit` command line
/// ([`spark_submit_words`]), found by walking that command's own grammar: an option word is
/// skipped, and so is the word after one of the options that takes its value there
/// ([`VALUE_OPTIONS`]). The words after the application are the application's own arguments,
/// which is what `spark-submit` passes it.
///
/// A command whose application is not a Python file in Amazon S3 names no application this
/// service holds an engine and bytes for ([`PYTHON_APPLICATION_SUFFIX`], [`s3_object`]).
#[must_use]
pub fn submitted_spark_application(jar: &str, args: &[String]) -> Option<SparkApplication> {
    let words = spark_submit_words(jar, args)?;
    let mut rest = words.iter().skip(1);
    let mut application: Option<&String> = None;
    while let Some(word) = rest.next() {
        if word.starts_with('-') {
            if VALUE_OPTIONS.contains(&word.as_str()) {
                // The value of this option lives in the next word, which is therefore not
                // the application.
                rest.next();
            }
            continue;
        }
        application = Some(word);
        break;
    }
    let path = application?;
    if !path.ends_with(PYTHON_APPLICATION_SUFFIX) {
        return None;
    }
    Some(SparkApplication {
        path: path.clone(),
        script: s3_object(path)?,
        arguments: rest.cloned().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row: the jar and the command a step submits, and what the reading answers — the
    /// bucket and key of the application and the arguments passed to it.
    struct Case {
        label: &'static str,
        jar: &'static str,
        args: Vec<String>,
        reads: Option<(&'static str, &'static str, Vec<String>)>,
    }

    /// The reading of a command line: which word is the application and which words are its
    /// own. The rows span the forms the grammar generalises over — options before the
    /// application, an option carrying its value in the same word, the shell-wrapped form,
    /// an application that is a jar rather than a Python file, a Python file on a node's own
    /// disk, and a command that is no `spark-submit` at all.
    #[test]
    fn a_step_command_names_the_application_it_submits() {
        let line = |words: &[&str]| -> Vec<String> {
            words.iter().map(|word| (*word).to_string()).collect()
        };
        let cases: Vec<Case> = vec![
            Case {
                label: "a bare submit of an object",
                jar: COMMAND_RUNNER_JAR,
                args: line(&["spark-submit", "s3://bucket/job.py"]),
                reads: Some(("bucket", "job.py", Vec::new())),
            },
            Case {
                label: "options before the application, arguments after it",
                jar: COMMAND_RUNNER_JAR,
                args: line(&[
                    "spark-submit",
                    "--deploy-mode",
                    "cluster",
                    "--conf",
                    "spark.executor.memory=2g",
                    "s3a://bucket/prefix/job.py",
                    "s3://bucket/out.csv",
                    "17",
                ]),
                reads: Some((
                    "bucket",
                    "prefix/job.py",
                    line(&["s3://bucket/out.csv", "17"]),
                )),
            },
            Case {
                label: "an option carrying its value in the same word, and a flag",
                jar: COMMAND_RUNNER_JAR,
                args: line(&[
                    "spark-submit",
                    "--conf=spark.sql.shuffle.partitions=4",
                    "--verbose",
                    "s3://bucket/job.py",
                ]),
                reads: Some(("bucket", "job.py", Vec::new())),
            },
            Case {
                label: "the shell-wrapped command line",
                jar: COMMAND_RUNNER_JAR,
                args: line(&[
                    "bash",
                    "-c",
                    "spark-submit --name 'a job' s3://bucket/job.py one",
                ]),
                reads: Some(("bucket", "job.py", line(&["one"]))),
            },
            Case {
                label: "a jar application, which no engine here runs",
                jar: COMMAND_RUNNER_JAR,
                args: line(&[
                    "spark-submit",
                    "--class",
                    "org.example.Main",
                    "s3://bucket/job.jar",
                ]),
                reads: None,
            },
            Case {
                label: "a Python file on a node's own disk, whose bytes this emulator has none of",
                jar: COMMAND_RUNNER_JAR,
                args: line(&["spark-submit", "/home/hadoop/job.py"]),
                reads: None,
            },
            Case {
                label: "a command that submits nothing",
                jar: COMMAND_RUNNER_JAR,
                args: line(&["hadoop", "fs", "-ls", "s3://bucket/"]),
                reads: None,
            },
            Case {
                label: "a jar of the submitter's own, not the command runner",
                jar: "s3://bucket/own.jar",
                args: line(&["spark-submit", "s3://bucket/job.py"]),
                reads: None,
            },
        ];
        for case in &cases {
            let label = case.label;
            let read = submitted_spark_application(case.jar, &case.args);
            let answered = read.as_ref().map(|application| {
                (
                    application.script.bucket.as_str(),
                    application.script.key.as_str(),
                    application.arguments.clone(),
                )
            });
            assert_eq!(
                answered, case.reads,
                "case {label:?}: expected {:?}, got {answered:?}",
                case.reads
            );
        }
    }
}
