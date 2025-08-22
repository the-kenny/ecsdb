use std::path::PathBuf;

use clap::*;
use ecsdb::*;
use rustyline::error::ReadlineError;
use tracing::debug;

#[derive(clap::Parser, Debug)]
struct Cli {
    filename: Option<PathBuf>,
    command: Option<String>,
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
        Some(ref path) => ecsdb::Ecs::open(path)?,
        None => ecsdb::Ecs::open_in_memory()?,
    };

    debug!("Opened DB");

    let mut rl = rustyline::DefaultEditor::new()?;

    const COMMANDS: Commands = &[&Info];

    if let Some(command) = cli.command {
        debug!(?command, "Executing");

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
            Err(ReadlineError::Eof) => {
                println!("Exiting...");
                return Ok(());
            }
            Err(_) => println!("No input"),
        }
    }
}

fn eval(commands: &Commands, db: &Ecs, line: &str) -> Result<(), CommandError> {
    let Some(command) = line.split_whitespace().next() else {
        return Ok(());
    };

    let Some(command) = commands.iter().find(|c| c.name() == command) else {
        println!("Command '{command}' not found");
        return Ok(());
    };

    command.execute(&db, &line)?;

    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Database(#[from] ecsdb::Error),
}

trait Command: std::fmt::Debug {
    fn name(&self) -> &'static str;
    fn execute(&self, db: &Ecs, input: &str) -> Result<(), CommandError>;
}

#[derive(Debug)]
struct Info;

impl Command for Info {
    fn name(&self) -> &'static str {
        ".info"
    }

    fn execute(&self, db: &Ecs, _input: &str) -> Result<(), CommandError> {
        let db_path = match db.raw_sql().path() {
            None => "???",
            Some("") => ":memory:",
            Some(path) => path,
        };

        println!("Database {}, data_version {}", db_path, db.data_version()?);
        Ok(())
    }
}
