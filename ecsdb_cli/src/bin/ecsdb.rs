use std::path::PathBuf;

use clap::*;
use ecsdb::*;

use ecsdb_cli::{COMMANDS, Command, CommandError, Commands};
use tracing::{debug, error, info_span, warn};

#[derive(clap::Parser, Debug)]
struct Cli {
    filename: Option<PathBuf>,
    command: Option<String>,

    #[clap(long, default_value = "false")]
    readonly: bool,
}

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
