// SPDX-License-Identifier: CC0-1.0

//! WHO a session on a cache's socket is, and WHAT it may do — the RBAC rules of the user
//! it authenticated as, read off the `AccessString` the control plane stored.
//!
//! `User$AccessString` is "access permissions string used for this user", written in the
//! Valkey / Redis OSS ACL rule syntax: a list of rules applied in order, each either a
//! state (`on` / `off`), a password rule (`nopass`), a key pattern (`~pattern`,
//! `allkeys`, `resetkeys`), a channel pattern (`&pattern`, `allchannels`) or a command
//! rule (`+@category`, `-@category`, `+command`, `-command`, `allcommands`,
//! `nocommands`). The engine reads a later rule as overriding an earlier one, so a list is
//! evaluated as the ORDERED thing it is rather than as a set.
//!
//! The same string the control plane validates ([`crate::state::access_string_rejection`])
//! is what decides a session's permissions here, so a user's access string means on the
//! socket what the API said it means.

use std::collections::BTreeSet;

/// What one rule of an access string targets.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// Every command (`@all`).
    Everything,
    /// One command category (`@read`, `@write`, …).
    Category(String),
    /// One command by name.
    Command(String),
}

/// One command rule of an access string: whether it allows or denies, and what it targets.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandRule {
    allow: bool,
    target: Target,
}

/// WHICH way a key pattern may be used: a plain `~pattern` reaches a key both ways, while
/// `%R~pattern` and `%W~pattern` reach it for reads or for writes alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Read and write (`~p`, `%RW~p`).
    Both,
    /// Reads only (`%R~p`).
    Read,
    /// Writes only (`%W~p`).
    Write,
}

impl Direction {
    /// Whether a pattern granted this way covers a command that does or does not WRITE.
    fn covers(self, writes: bool) -> bool {
        match self {
            Self::Both => true,
            Self::Read => !writes,
            Self::Write => writes,
        }
    }
}

/// The permissions one user's access string gives, in the order its rules were written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessRules {
    /// `on` / `off`: whether the user may be authenticated at all. An `off` user is
    /// refused however correct its credential is.
    pub enabled: bool,
    /// `nopass`: the user authenticates with any password, which is what a cache with no
    /// credential of its own serves.
    pub no_password: bool,
    /// Whether every key is reachable (`allkeys` / `~*`).
    all_keys: bool,
    /// The key patterns the user may reach, each with the direction it was granted for.
    key_patterns: Vec<(Direction, String)>,
    /// The command rules, in the order written: the LAST one that matches a command is the
    /// one that decides it.
    command_rules: Vec<CommandRule>,
}

impl AccessRules {
    /// The permissions the access string `access_string` gives.
    pub fn parse(access_string: &str) -> Self {
        let mut rules = Self::default();
        for rule in access_string.split_whitespace() {
            rules.apply(rule);
        }
        rules
    }

    /// The permissions a session that authenticated with the cache's own auth token has:
    /// the whole keyspace and every command, which is what a cache whose access control is
    /// one shared token gives a client that holds it.
    pub fn unrestricted() -> Self {
        Self {
            enabled: true,
            no_password: false,
            all_keys: true,
            key_patterns: Vec::new(),
            command_rules: vec![CommandRule {
                allow: true,
                target: Target::Everything,
            }],
        }
    }

    /// Apply one rule of an access string, in the order the string wrote it.
    fn apply(&mut self, rule: &str) {
        let lowered = rule.to_ascii_lowercase();
        match lowered.as_str() {
            "on" => self.enabled = true,
            "off" => self.enabled = false,
            "nopass" => self.no_password = true,
            "resetpass" => self.no_password = false,
            "allkeys" => {
                self.all_keys = true;
                self.key_patterns.clear();
            }
            "resetkeys" => {
                self.all_keys = false;
                self.key_patterns.clear();
            }
            "allcommands" => self.set_default(true),
            "nocommands" => self.set_default(false),
            // A channel rule governs pub/sub, which this data plane does not serve, so it
            // is read and carried by nothing.
            "allchannels" | "resetchannels" => {}
            _ => self.apply_prefixed(rule),
        }
    }

    /// Apply a rule that carries a value behind its prefix: a key or channel pattern, or a
    /// command or category rule.
    fn apply_prefixed(&mut self, rule: &str) {
        let mut characters = rule.chars();
        match characters.next() {
            Some('~') => {
                let pattern: String = characters.collect();
                self.grant_pattern(Direction::Both, &pattern);
            }
            // `%R~p` / `%W~p` / `%RW~p` narrow a pattern to reads or to writes, which is
            // the direction the key is reachable in.
            Some('%') => {
                let Some((prefix, pattern)) = rule.split_once('~') else {
                    return;
                };
                let direction = match prefix.trim_start_matches('%').to_ascii_uppercase().as_str()
                {
                    "R" => Direction::Read,
                    "W" => Direction::Write,
                    "RW" | "" => Direction::Both,
                    // A direction this build does not know grants nothing, rather than
                    // granting more than the rule asked for.
                    _ => return,
                };
                self.grant_pattern(direction, pattern);
            }
            Some('&') => {}
            Some(sign @ ('+' | '-')) => {
                let allow = sign == '+';
                let named: String = characters.collect();
                let target = match named.strip_prefix('@') {
                    Some("all") => Target::Everything,
                    Some(category) => Target::Category(category.to_ascii_lowercase()),
                    None => Target::Command(named.to_ascii_lowercase()),
                };
                if target == Target::Everything {
                    self.set_default(allow);
                } else {
                    self.command_rules.push(CommandRule { allow, target });
                }
            }
            _ => {}
        }
    }

    /// Give the user the key pattern `pattern`, reachable `direction`. `~*` both ways is
    /// every key, which is what `allkeys` means.
    fn grant_pattern(&mut self, direction: Direction, pattern: &str) {
        if pattern == "*" && direction == Direction::Both {
            self.all_keys = true;
            self.key_patterns.clear();
            return;
        }
        self.key_patterns.push((direction, pattern.to_string()));
    }

    /// `+@all` / `-@all`: the baseline every later rule is read against, which replaces
    /// every rule written before it.
    fn set_default(&mut self, allow: bool) {
        self.command_rules.clear();
        self.command_rules.push(CommandRule {
            allow,
            target: Target::Everything,
        });
    }

    /// Whether this user may run the command `command`, which belongs to `categories`.
    ///
    /// The LAST rule that matches decides, which is how the engine reads a rule list; a
    /// command no rule matches is denied, because a user is given permissions rather than
    /// starting with them.
    pub fn allows_command(&self, command: &str, categories: &[&str]) -> bool {
        let command = command.to_ascii_lowercase();
        let categories: BTreeSet<&str> = categories.iter().copied().collect();
        self.command_rules
            .iter()
            .rev()
            .find(|rule| match &rule.target {
                Target::Everything => true,
                Target::Category(category) => categories.contains(category.as_str()),
                Target::Command(named) => *named == command,
            })
            .is_some_and(|rule| rule.allow)
    }

    /// Whether this user may reach the key `key` for a command that does or does not
    /// WRITE: every key when it holds `allkeys`, and otherwise a key one of its patterns
    /// matches in a direction that covers the command — read as the engine reads a pattern
    /// ([`super::keyspace::glob_matches`]), so a user given `%R~p` may read those keys and
    /// not write them.
    pub fn allows_key(&self, key: &str, writes: bool) -> bool {
        self.all_keys
            || self.key_patterns.iter().any(|(direction, pattern)| {
                direction.covers(writes) && super::keyspace::glob_matches(pattern, key)
            })
    }
}
