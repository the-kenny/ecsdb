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
        &QueryCommand,
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

// --- Pipe-based query language ---

mod pipeline {

    use ecsdb::{Ecs, Entity};
    use nom::bytes::complete::take_while;
    use serde_json::json;
    use tracing::warn;

    #[derive(Debug)]
    pub struct Pipeline {
        pub stages: Vec<Stage>,
    }

    impl Pipeline {
        pub fn parse(input: &str) -> Result<Self, ParseError> {
            use nom::{
                IResult, Parser,
                branch::alt,
                bytes::complete::{tag, tag_no_case},
                character::complete::{char, digit1, multispace0},
                combinator::{map, opt},
                multi::separated_list1,
                sequence::{delimited, preceded},
            };

            fn ws<'a, O>(
                inner: impl Parser<&'a str, Output = O, Error = nom::error::Error<&'a str>>,
            ) -> impl Parser<&'a str, Output = O, Error = nom::error::Error<&'a str>> {
                delimited(multispace0, inner, multispace0)
            }

            fn single_quoted_string(input: &str) -> IResult<&str, &str> {
                let (input, _) = char('\'').parse(input)?;
                let (input, s) = take_while(|c| c != '\'').parse(input)?;
                let (input, _) = char('\'').parse(input)?;
                Ok((input, s))
            }

            fn double_quoted_string(input: &str) -> IResult<&str, &str> {
                let (input, _) = char('"').parse(input)?;
                let (input, s) = take_while(|c| c != '"').parse(input)?;
                let (input, _) = char('"').parse(input)?;
                Ok((input, s))
            }

            fn quoted_string(input: &str) -> IResult<&str, &str> {
                alt((single_quoted_string, double_quoted_string)).parse(input)
            }

            fn float_literal(input: &str) -> IResult<&str, FilterValue> {
                let (input, int_part) = digit1.parse(input)?;
                let (input, _) = char('.').parse(input)?;
                let (input, frac_part) = digit1.parse(input)?;
                let f: f64 = format!("{int_part}.{frac_part}").parse().unwrap();
                Ok((input, FilterValue(json!(f))))
            }

            fn value_literal(input: &str) -> IResult<&str, FilterValue> {
                alt((
                    map(tag("null"), |_| FilterValue(serde_json::Value::Null)),
                    map(tag("true"), |_| FilterValue(serde_json::Value::Bool(true))),
                    map(tag("false"), |_| {
                        FilterValue(serde_json::Value::Bool(false))
                    }),
                    map(quoted_string, |s: &str| {
                        FilterValue(serde_json::Value::String(s.to_owned()))
                    }),
                    float_literal,
                    map(digit1, |n: &str| {
                        FilterValue(serde_json::Value::Number(n.parse().unwrap()))
                    }),
                ))
                .parse(input)
            }

            fn cmp_op(input: &str) -> IResult<&str, FilterOperator> {
                alt((
                    map(tag("=="), |_| FilterOperator::Eq),
                    map(tag("="), |_| FilterOperator::Eq),
                    map(tag("!="), |_| FilterOperator::Ne),
                    map(tag("<="), |_| FilterOperator::Le),
                    map(tag(">="), |_| FilterOperator::Ge),
                    map(tag("<"), |_| FilterOperator::Lt),
                    map(tag(">"), |_| FilterOperator::Gt),
                ))
                .parse(input)
            }

            fn filter_column(input: &str) -> IResult<&str, FilterColumn> {
                alt((
                    map(tag("entity"), |_| FilterColumn::Entity),
                    map(tag("component"), |_| FilterColumn::Component),
                    map(tag("data"), |_| FilterColumn::Data),
                ))
                .parse(input)
            }

            fn comparison(input: &str) -> IResult<&str, FilterExpression> {
                let (input, col) = ws(filter_column).parse(input)?;
                let (input, op) = ws(cmp_op).parse(input)?;
                let (input, val) = ws(value_literal).parse(input)?;
                Ok((
                    input,
                    FilterExpression::Eq {
                        column: col,
                        op,
                        value: val,
                    },
                ))
            }

            fn filter_atom(input: &str) -> IResult<&str, FilterExpression> {
                let (input, _) = multispace0.parse(input)?;
                alt((
                    delimited(ws(char('(')), filter_or, ws(char(')'))),
                    comparison,
                ))
                .parse(input)
            }

            fn filter_and(input: &str) -> IResult<&str, FilterExpression> {
                let (input, first) = filter_atom(input)?;
                let (input, rest) =
                    nom::multi::many0(preceded(ws(tag("&&")), filter_atom)).parse(input)?;
                Ok((
                    input,
                    rest.into_iter().fold(first, |acc, next| {
                        FilterExpression::And(Box::new(acc), Box::new(next))
                    }),
                ))
            }

            fn filter_or(input: &str) -> IResult<&str, FilterExpression> {
                let (input, first) = filter_and(input)?;
                let (input, rest) =
                    nom::multi::many0(preceded(ws(tag("||")), filter_and)).parse(input)?;
                Ok((
                    input,
                    rest.into_iter().fold(first, |acc, next| {
                        FilterExpression::Or(Box::new(acc), Box::new(next))
                    }),
                ))
            }

            fn filter_stage(input: &str) -> IResult<&str, Stage> {
                let (input, _) = ws(tag("filter")).parse(input)?;
                let (input, expr) = delimited(char('('), ws(filter_or), char(')')).parse(input)?;
                Ok((input, Stage::Filter(expr)))
            }

            fn sort_stage(input: &str) -> IResult<&str, Stage> {
                let (input, _) = ws(tag_no_case("sortBy")).parse(input)?;
                let (input, _) = char('(').parse(input)?;
                let (input, _) = multispace0.parse(input)?;
                let (input, field) = alt((
                    map(tag_no_case("created_at"), |_| SortField::CreatedAt),
                    map(tag_no_case("last_modified"), |_| SortField::LastModified),
                    map(filter_column, SortField::Column),
                ))
                .parse(input)?;
                let (input, _) = multispace0.parse(input)?;
                let (input, order) = opt(alt((
                    map(tag_no_case("asc"), |_| SortOrder::Asc),
                    map(tag_no_case("desc"), |_| SortOrder::Desc),
                )))
                .parse(input)?;
                let (input, _) = multispace0.parse(input)?;
                let (input, _) = char(')').parse(input)?;
                Ok((input, Stage::SortBy(field, order.unwrap_or(SortOrder::Asc))))
            }

            fn take_stage(input: &str) -> IResult<&str, Stage> {
                let (input, _) = ws(tag("take")).parse(input)?;
                let (input, n) = delimited(char('('), ws(digit1), char(')')).parse(input)?;
                let n: usize = n.parse().unwrap();
                Ok((input, Stage::Take(n)))
            }

            fn skip_stage(input: &str) -> IResult<&str, Stage> {
                let (input, _) = ws(tag("skip")).parse(input)?;
                let (input, n) = delimited(char('('), ws(digit1), char(')')).parse(input)?;
                let n: usize = n.parse().unwrap();
                Ok((input, Stage::Skip(n)))
            }

            fn stage(input: &str) -> IResult<&str, Stage> {
                alt((
                    map(ws(tag("all")), |_| Stage::All),
                    filter_stage,
                    sort_stage,
                    take_stage,
                    skip_stage,
                ))
                .parse(input)
            }

            fn parse_pipeline(input: &str) -> IResult<&str, Pipeline> {
                let (input, stages) = separated_list1(ws(char('|')), stage).parse(input)?;
                let (input, _) = multispace0.parse(input)?;
                Ok((input, Pipeline { stages }))
            }

            let (remaining, pipe) =
                parse_pipeline(input).map_err(|e| ParseError(format!("{e}")))?;

            if !remaining.trim().is_empty() {
                return Err(ParseError(format!(
                    "Unexpected trailing input: '{remaining}'"
                )));
            }

            Ok(pipe)
        }
    }

    #[derive(thiserror::Error, Debug)]
    #[error("Parse Error: {0}")]
    pub struct ParseError(pub String);

    #[derive(Debug)]
    pub enum Stage {
        All,
        Filter(FilterExpression),
        SortBy(SortField, SortOrder),
        Take(usize),
        Skip(usize),
    }

    impl Stage {
        pub fn apply<'a>(
            self,
            db: &'a Ecs,
            chain: impl Iterator<Item = Row<'a>> + 'a,
        ) -> impl Iterator<Item = Row<'a>> + 'a {
            match self {
                Stage::All => {
                    let rows = db.query::<Entity, ()>().flat_map(|entity| {
                        entity
                            .component_names()
                            .map(move |component| Row { entity, component })
                    });
                    Box::new(rows) as Box<dyn Iterator<Item = _>>
                }
                Stage::Filter(filter) => {
                    Box::new(filter.apply(chain)) as Box<dyn Iterator<Item = _>>
                }
                Stage::SortBy(field, order) => {
                    let mut intermediate: Vec<_> = chain.collect();
                    intermediate.sort_unstable_by(|a, b| {
                        let a = (
                            field.extract(a).0.to_string(),
                            a.entity.id(),
                            a.component.as_str(),
                        );
                        let b = (
                            field.extract(b).0.to_string(),
                            b.entity.id(),
                            b.component.as_str(),
                        );
                        let result = a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
                        match order {
                            SortOrder::Asc => result,
                            SortOrder::Desc => result.reverse(),
                        }
                    });
                    Box::new(intermediate.into_iter())
                }
                Stage::Take(n) => Box::new(chain.take(n)),
                Stage::Skip(n) => Box::new(chain.skip(n)),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub enum SortField {
        Column(FilterColumn),
        CreatedAt,
        LastModified,
    }

    impl SortField {
        fn extract(self, row: &Row<'_>) -> FilterValue {
            match self {
                Self::Column(column) => column.extract(row),
                Self::CreatedAt => FilterValue(serde_json::Value::String(
                    row.entity.created_at().to_rfc3339(),
                )),
                Self::LastModified => FilterValue(serde_json::Value::String(
                    row.entity.last_modified().to_rfc2822(),
                )),
            }
        }
    }

    #[derive(Debug)]
    pub enum SortOrder {
        Asc,
        Desc,
    }

    #[derive(Debug, PartialEq)]
    pub struct FilterValue(serde_json::Value);

    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub enum FilterColumn {
        Entity,
        Component,
        Data,
    }

    impl FilterColumn {
        #[tracing::instrument(level = "debug")]
        fn extract(self, row: &Row) -> FilterValue {
            match self {
                FilterColumn::Entity => {
                    FilterValue(serde_json::Value::Number(row.entity.id().into()))
                }
                FilterColumn::Component => {
                    FilterValue(serde_json::Value::String(row.component.clone()))
                }
                FilterColumn::Data => {
                    let Some(c) = row.entity.dyn_component(&row.component) else {
                        warn!("Failed to get DynComponent");
                        return FilterValue(serde_json::Value::Null);
                    };

                    match c.kind() {
                        ecsdb::dyn_component::Kind::Json => FilterValue(c.as_json().unwrap()),
                        ecsdb::dyn_component::Kind::Blob => {
                            warn!("'data' filter on BLOB is unimplemented. Row will be skipped");
                            FilterValue(serde_json::Value::Null)
                        }
                        ecsdb::dyn_component::Kind::Null => FilterValue(serde_json::Value::Null),
                        ecsdb::dyn_component::Kind::Other(r#type) => {
                            unimplemented!("Filter on {:?}", r#type)
                        }
                    }
                }
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FilterOperator {
        Eq,
        Ne,
        Lt,
        Le,
        Gt,
        Ge,
    }

    #[derive(Debug)]
    pub enum FilterExpression {
        Eq {
            column: FilterColumn,
            op: FilterOperator,
            value: FilterValue,
        },
        And(Box<FilterExpression>, Box<FilterExpression>),
        Or(Box<FilterExpression>, Box<FilterExpression>),
    }

    impl FilterExpression {
        pub fn matches(&self, row: &Row<'_>) -> bool {
            match self {
                Self::Eq {
                    column,
                    op: FilterOperator::Eq,
                    value,
                } => column.extract(row) == *value,
                Self::Eq {
                    column,
                    op: FilterOperator::Ne,
                    value,
                } => column.extract(row) != *value,
                Self::Eq { .. } => todo!("{self:?}"),
                Self::And(a, b) => a.matches(row) && b.matches(row),
                Self::Or(a, b) => a.matches(row) || b.matches(row),
            }
        }

        pub fn apply<'a, C>(self, chain: C) -> impl Iterator<Item = C::Item>
        where
            C: Iterator<Item = Row<'a>>,
        {
            chain.filter(move |row| self.matches(row))
        }
    }

    /// A row in the pipeline — always a full entity with all its components.
    #[derive(Debug)]
    pub struct Row<'a> {
        entity: ecsdb::Entity<'a>,
        component: String,
    }

    impl<'a> Row<'a> {
        pub fn entity(&self) -> ecsdb::Entity<'_> {
            self.entity
        }

        pub fn component_name(&self) -> &str {
            &self.component
        }

        pub fn component_value(&self) -> impl std::fmt::Display {
            self.entity
                .dyn_component(&self.component)
                .and_then(|c| c.as_json())
                .unwrap_or_default()
        }
    }
}

#[derive(Debug)]
struct QueryCommand;

impl QueryCommand {
    fn execute_pipeline(db: &Ecs, input: &str) -> Result<(), anyhow::Error> {
        use pipeline::*;

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

#[cfg(test)]
mod test {
    use insta::assert_debug_snapshot;

    use super::pipeline::*;

    const PIPELINES: &[&str] = &[
        "all",
        "all|filter(entity == 42)",
        "all|filter(entity = 42)",
        "all|filter(entity != 42)",
        "all | filter(entity == 42) | filter(entity == 23)",
        "all | take(23)",
        "all | skip(1)",
        "all | skip(1) | sortBy(last_modified)",
    ];

    #[test]
    fn query_filters() {
        for &pipeline in PIPELINES {
            insta::with_settings!({ snapshot_suffix => pipeline, description => format!("Pipeline::parse(\"{pipeline}\")") }, {
                assert_debug_snapshot!(Pipeline::parse(pipeline))
            });
        }
    }
}
