// SPDX-License-Identifier: CC0-1.0

//! The ACCESS CONTROL LIST a PostgreSQL object carries its privileges in, and the catalog
//! row it lives on.
//!
//! PostgreSQL holds the privileges granted on an object ON THAT OBJECT, as an array of
//! `aclitem` values written `grantee=privileges/grantor` (an empty grantee for `PUBLIC`) —
//! `pg_class.relacl` for a table or view, `pg_namespace.nspacl` for a schema,
//! `pg_database.datacl` for a database. This is that text, read and written: the one place
//! this server spells it, so a grant and a revoke cannot disagree about what a list means,
//! and a client reads back exactly what it granted.

use super::*;

/// The access control list of the object `name`, or `None` when this instance holds no
/// such object. The outer `Option` is the object, the inner one its list, which is unset
/// for an object still carrying only its owner's default privileges.
pub(super) fn object_acl(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    class: ObjectClass,
    name: &str,
) -> Result<Option<Option<String>>, EngineError> {
    let (catalog, name_column, acl_column) = class.catalog();
    let found = database.execute_in(
        facts.connection_id,
        &facts.database,
        &format!("SELECT {acl_column} FROM {catalog} WHERE {name_column} = ?"),
        &[Some(name.to_string())],
    )?;
    Ok(found.rows.first().map(|row| match row.first() {
        Some(Cell::Text(acl)) => Some(acl.clone()),
        _ => None,
    }))
}

/// The tables the schema `schema` holds, as `pg_class` reports them.
pub(super) fn tables_in_schema(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    schema: &str,
) -> Result<Vec<String>, EngineError> {
    let found = database.execute_in(
        facts.connection_id,
        &facts.database,
        "SELECT c.relname FROM pg_class AS c \
           JOIN pg_namespace AS n ON n.oid = c.relnamespace \
         WHERE n.nspname = ? AND c.relkind = 'r' ORDER BY c.relname",
        &[Some(schema.to_string())],
    )?;
    Ok(found
        .rows
        .iter()
        .filter_map(|row| match row.first() {
            Some(Cell::Text(name)) => Some(name.clone()),
            _ => None,
        })
        .collect())
}

/// One object's access control list, as PostgreSQL's `aclitem[]` renders it:
/// `{grantee=privileges/grantor}`, with an empty grantee for `PUBLIC`.
#[derive(Debug, Default)]
pub(super) struct AccessList {
    /// One entry per grantee, in the order the list carries them.
    pub(super) entries: Vec<AccessEntry>,
}

/// One entry of an access control list.
#[derive(Debug)]
pub(super) struct AccessEntry {
    pub(super) grantee: String,
    /// The privilege letters held, in [`PRIVILEGES`] order.
    pub(super) privileges: Vec<char>,
    pub(super) grantor: String,
}

impl AccessList {
    /// Read a stored list, which is absent for an object nothing has been granted on.
    pub(super) fn read(stored: Option<&str>) -> Self {
        let Some(stored) = stored else {
            return Self::default();
        };
        let body = stored
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .to_string();
        let entries = body
            .split(',')
            .filter(|item| !item.trim().is_empty())
            .filter_map(|item| {
                let (grantee, rest) = item.trim().split_once('=')?;
                let (privileges, grantor) = rest.split_once('/').unwrap_or((rest, ""));
                Some(AccessEntry {
                    grantee: grantee.to_string(),
                    privileges: privileges.chars().collect(),
                    grantor: grantor.to_string(),
                })
            })
            .collect();
        Self { entries }
    }

    /// Add `letters` to `grantee`'s entry, creating it where there is none.
    pub(super) fn grant(&mut self, grantee: &str, grantor: &str, letters: &[char]) {
        if !self.entries.iter().any(|entry| entry.grantee == grantee) {
            self.entries.push(AccessEntry {
                grantee: grantee.to_string(),
                privileges: Vec::new(),
                grantor: grantor.to_string(),
            });
        }
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.grantee == grantee)
        else {
            return;
        };
        for letter in letters {
            if !entry.privileges.contains(letter) {
                entry.privileges.push(*letter);
            }
        }
        entry.privileges.sort_by_key(|letter| {
            PRIVILEGES
                .iter()
                .position(|privilege| privilege.letter == *letter)
                .unwrap_or(usize::MAX)
        });
    }

    /// Take `letters` off `grantee`'s entry, dropping an entry left holding nothing —
    /// which is how PostgreSQL leaves a list a revoke emptied.
    pub(super) fn revoke(&mut self, grantee: &str, letters: &[char]) {
        for entry in &mut self.entries {
            if entry.grantee == grantee {
                entry.privileges.retain(|held| !letters.contains(held));
            }
        }
        self.entries.retain(|entry| !entry.privileges.is_empty());
    }

    /// The list as a column value: `None` for a list holding nothing, which is the state
    /// an object with no granted privileges is in.
    pub(super) fn write(&self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let items: Vec<String> = self
            .entries
            .iter()
            .map(|entry| {
                format!(
                    "{}={}/{}",
                    entry.grantee,
                    entry.privileges.iter().collect::<String>(),
                    entry.grantor
                )
            })
            .collect();
        Some(format!("{{{}}}", items.join(",")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The access control list reads and writes PostgreSQL's own `aclitem` text, and a
    /// grant merges into the entry a grantee already holds rather than replacing it: the
    /// rows differ along the stored list, the grantee (`PUBLIC` included, which carries an
    /// empty name) and what is granted.
    #[test]
    fn an_access_list_round_trips_and_merges_by_grantee() {
        for (case, stored, grantee, letters, expected) in [
            (
                "an object with no list yet",
                None,
                "reader",
                vec!['r'],
                Some("{reader=r/dbadmin}".to_string()),
            ),
            (
                "a second privilege for the same grantee",
                Some("{reader=r/dbadmin}"),
                "reader",
                vec!['a'],
                Some("{reader=ar/dbadmin}".to_string()),
            ),
            (
                "a privilege already held",
                Some("{reader=ar/dbadmin}"),
                "reader",
                vec!['r'],
                Some("{reader=ar/dbadmin}".to_string()),
            ),
            (
                "another grantee",
                Some("{reader=r/dbadmin}"),
                "writer",
                vec!['w'],
                Some("{reader=r/dbadmin,writer=w/dbadmin}".to_string()),
            ),
            (
                "PUBLIC",
                None,
                "",
                vec!['r'],
                Some("{=r/dbadmin}".to_string()),
            ),
        ] {
            let mut acl = AccessList::read(stored);
            acl.grant(grantee, "dbadmin", &letters);
            let written = acl.write();
            assert_eq!(
                written, expected,
                "case {case:?}: expected {expected:?}, got {written:?}"
            );
        }
    }

    /// A revoke takes exactly the privileges it names off, and an entry left holding
    /// nothing is dropped.
    #[test]
    fn a_revoke_removes_what_it_names_and_drops_an_empty_entry() {
        for (case, stored, grantee, letters, expected) in [
            (
                "one of two privileges",
                "{reader=ar/dbadmin}",
                "reader",
                vec!['a'],
                Some("{reader=r/dbadmin}".to_string()),
            ),
            (
                "the last privilege",
                "{reader=r/dbadmin}",
                "reader",
                vec!['r'],
                None,
            ),
            (
                "a privilege not held",
                "{reader=r/dbadmin}",
                "reader",
                vec!['w'],
                Some("{reader=r/dbadmin}".to_string()),
            ),
            (
                "another grantee's entry",
                "{reader=r/dbadmin,writer=w/dbadmin}",
                "reader",
                vec!['r'],
                Some("{writer=w/dbadmin}".to_string()),
            ),
        ] {
            let mut acl = AccessList::read(Some(stored));
            acl.revoke(grantee, &letters);
            let written = acl.write();
            assert_eq!(
                written, expected,
                "case {case:?}: expected {expected:?}, got {written:?}"
            );
        }
    }
}
