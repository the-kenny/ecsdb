use std::{collections::BTreeSet, fmt::Display};

use ecsdb::*;

use tracing::debug;

pub type Commands<'a> = &'a [&'a dyn Command];

pub trait Command: std::fmt::Debug {
    fn name(&self) -> &'static str;
    fn execute(&self, db: &Ecs, input: &str) -> CommandResult;
}

pub type CommandResult = Result<(), CommandError>;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Database(#[from] ecsdb::Error),

    #[error(transparent)]
    CommandFailed(anyhow::Error),
}

#[derive(Debug)]
pub struct Info;

impl Command for Info {
    fn name(&self) -> &'static str {
        ".info"
    }

    fn execute(&self, db: &Ecs, _input: &str) -> CommandResult {
        let db_path = match db.raw_sql().path() {
            None => "???",
            Some("") => ":memory:",
            Some(path) => path,
        };

        println!("Database {}, data_version {}", db_path, db.data_version()?);
        Ok(())
    }
}

#[derive(Debug)]
pub struct Entities;

impl Command for Entities {
    fn name(&self) -> &'static str {
        "entities"
    }

    fn execute(&self, db: &Ecs, input: &str) -> CommandResult {
        if input.trim() != self.name() {
            println!("Ignoring arguments '{input}'");
        }

        for eid in db.try_query::<EntityId, ()>()? {
            println!("{eid}");
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct Components;

impl Command for Components {
    fn name(&self) -> &'static str {
        "components"
    }

    fn execute(&self, db: &Ecs, input: &str) -> CommandResult {
        if input.trim() != self.name() {
            println!("Ignoring arguments '{input}'");
        }

        let components: BTreeSet<_> = db
            .try_query::<Entity, ()>()?
            .flat_map(|e| e.component_names().collect::<Box<[_]>>())
            .collect();

        for component in components {
            println!("{component}");
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct RegisteredSystems;

impl Command for RegisteredSystems {
    fn name(&self) -> &'static str {
        "systems"
    }

    fn execute(&self, db: &Ecs, input: &str) -> CommandResult {
        if input.trim() != self.name() {
            println!("Ignoring arguments '{input}'");
        }

        let systems: BTreeSet<_> = db.try_query::<ecsdb::system::Name, ()>()?.collect();

        for system in systems {
            println!("{system}");
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct SqliteExecute;

impl SqliteExecute {
    fn run(db: &rusqlite::Connection, sql: &str) -> Result<(), rusqlite::Error> {
        let mut stmt = db.prepare(sql)?;

        let cols = stmt
            .column_names()
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();

        debug!(?cols);
        println!("{}", cols.join("\t| "));

        let mut rows = stmt.query([])?;

        while let Some(row) = rows.next()? {
            for col in &cols {
                let val = row.get_ref(col.as_str())?;
                let val: Box<dyn Display> = match val {
                    ecsdb::rusqlite::types::ValueRef::Null => Box::new("NULL"),
                    ecsdb::rusqlite::types::ValueRef::Integer(n) => Box::new(n),
                    ecsdb::rusqlite::types::ValueRef::Real(r) => Box::new(r),
                    ecsdb::rusqlite::types::ValueRef::Text(text) => Box::new(str::from_utf8(text)?),
                    ecsdb::rusqlite::types::ValueRef::Blob(items) => {
                        Box::new(format!("Blob<{} bytes>", items.len()))
                    }
                };
                print!("{val}\t")
            }

            println!();
        }

        Ok(())
    }
}

impl Command for SqliteExecute {
    fn name(&self) -> &'static str {
        ".sql"
    }

    fn execute(&self, db: &Ecs, input: &str) -> CommandResult {
        let sql = input.trim_start_matches(self.name()).trim();
        Self::run(db.raw_sql(), sql).map_err(|e| CommandError::CommandFailed(e.into()))?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct QueryCommand;

impl QueryCommand {
    fn execute_pipeline(db: &Ecs, input: &str) -> Result<(), anyhow::Error> {
        use super::pipeline::*;

        let pipeline = Pipeline::parse(input)?;

        debug!(?pipeline);

        let chain = pipeline.stages.into_iter().fold(
            Box::new(std::iter::empty()) as Box<dyn Iterator<Item = Row>>,
            |chain, link: Stage| Box::new(link.apply(db, chain)),
        );

        for row in chain {
            println!(
                "{:>5} | {:30} | {:5>}",
                row.entity().id(),
                row.component_name(),
                row.component_value()
            );
        }

        Ok(())
    }
}

impl Command for QueryCommand {
    fn name(&self) -> &'static str {
        "query"
    }

    fn execute(&self, db: &Ecs, input: &str) -> CommandResult {
        let query_input = input.trim_start_matches(self.name()).trim();
        if query_input.is_empty() {
            println!("Usage: query all | filter(component = 'Foo') | sortBy(entity) | take(10)");
            return Ok(());
        }
        Self::execute_pipeline(db, query_input).map_err(CommandError::CommandFailed)?;
        Ok(())
    }
}
