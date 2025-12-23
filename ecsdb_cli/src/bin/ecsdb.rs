use std::{collections::BTreeSet, fmt::Display, path::PathBuf};

use clap::*;
use ecsdb::*;

use tracing::{debug, error, info_span, warn};

#[derive(clap::Parser, Debug)]
struct Cli {
    filename: Option<PathBuf>,
    command: Option<String>,

    #[clap(long, default_value = "false")]
    readonly: bool,
}

type Commands<'a> = &'a [&'a dyn Command];

pub fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    debug!(?cli);

    let _span = tracing::debug_span!(
        "db",
        path = cli
            .filename
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or(":memory:".into())
    )
    .entered();

    let db = match cli.filename {
        Some(ref path) => {
            use ecsdb::rusqlite::OpenFlags;
            let mut flags = OpenFlags::default();
            if cli.readonly {
                flags -= OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
                flags |= OpenFlags::SQLITE_OPEN_READ_ONLY;
            }

            println!("Opening {}", path.display());

            ecsdb::Ecs::open_with_flags(path, flags)?
        }
        None => {
            println!("Using in-memory database");
            ecsdb::Ecs::open_in_memory()?
        }
    };

    debug!("Opened DB");

    let mut rl = {
        let config = rustyline::config::Config::builder()
            .auto_add_history(true)
            .build();

        let history = if let Some(config_dir) = dirs::data_dir() {
            rustyline::sqlite_history::SQLiteHistory::open(
                &config,
                &config_dir.join("ecsdb_history.sqlite3"),
            )?
        } else {
            warn!("Couldn't retrieve data directory. History will not be persisted.");
            rustyline::sqlite_history::SQLiteHistory::with_config(&config)?
        };

        let mut rl = rustyline::Editor::<
            CompletionHandler,
            rustyline::sqlite_history::SQLiteHistory,
        >::with_history(config, history)?;

        let hinter = CompletionHandler { commands: COMMANDS };
        rl.set_helper(Some(hinter));
        rl
    };

    const COMMANDS: Commands = &[
        &Info,
        &SqliteExecute,
        &Entities,
        &Components,
        &RegisteredSystems,
    ];

    if let Some(command) = cli.command {
        let _span = info_span!("command", ?command).entered();

        debug!("executing");

        eval(&COMMANDS, &db, &command)?;
        return Ok(());
    }

    debug!("Entering REPL");

    loop {
        let readline = rl.readline(">> ");
        match readline {
            Ok(line) => {
                eval(&COMMANDS, &db, &line)?;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("Exiting...");
                return Ok(());
            }
            Err(_) => println!("No input"),
        }
    }
}

struct CompletionHandler<'a> {
    commands: &'a [&'a dyn Command],
}

impl rustyline::validate::Validator for CompletionHandler<'_> {}
impl rustyline::highlight::Highlighter for CompletionHandler<'_> {}
impl rustyline::Helper for CompletionHandler<'_> {}

impl rustyline::completion::Completer for CompletionHandler<'_> {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        assert!(pos <= line.len());

        if line.is_empty() || pos < line.len() {
            return Ok((0, Vec::with_capacity(0)));
        }

        let needle = &line[..pos];
        let candidates = self
            .commands
            .iter()
            .filter(|c| c.name().starts_with(needle))
            .map(|c| {
                let suffix = &c.name()[pos..];
                format!("{suffix} ")
            })
            .collect::<Vec<_>>();

        Ok((pos, candidates))
    }
}

impl rustyline::hint::Hinter for CompletionHandler<'_> {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<Self::Hint> {
        if line.is_empty() || pos < line.len() {
            return None;
        }

        self.commands
            .iter()
            .filter_map(|hint| {
                // expect hint after word complete, like redis cli, add condition:
                // line.ends_with(" ")
                if hint.name().starts_with(line) {
                    Some(hint.name()[pos..].to_string())
                } else {
                    None
                }
            })
            .next()
    }
}

fn eval(commands: &Commands, db: &Ecs, line: &str) -> Result<(), ecsdb::Error> {
    let Some(command) = line.split_whitespace().next() else {
        return Ok(());
    };

    let Some(command) = commands.iter().find(|c| c.name() == command) else {
        println!("Command '{command}' not found");
        return Ok(());
    };

    match command.execute(db, line) {
        Ok(()) => Ok(()),
        Err(CommandError::Database(error)) => {
            error!(%error,"database error");
            Err(error)
        }
        Err(CommandError::CommandFailed(error)) => {
            warn!(%error, "Failed to execute command");
            eprintln!("Execution failed: {error}");
            Ok(())
        }
    }
}

pub type CommandResult = Result<(), CommandError>;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Database(#[from] ecsdb::Error),

    #[error(transparent)]
    CommandFailed(anyhow::Error),
}

trait Command: std::fmt::Debug {
    fn name(&self) -> &'static str;
    fn execute(&self, db: &Ecs, input: &str) -> CommandResult;
}

#[derive(Debug)]
struct Info;

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
struct Entities;

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
struct Components;

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
struct RegisteredSystems;

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
struct SqliteExecute;

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
                    rusqlite::types::ValueRef::Null => Box::new("NULL"),
                    rusqlite::types::ValueRef::Integer(n) => Box::new(n),
                    rusqlite::types::ValueRef::Real(r) => Box::new(r),
                    rusqlite::types::ValueRef::Text(text) => Box::new(str::from_utf8(text)?),
                    rusqlite::types::ValueRef::Blob(items) => {
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
