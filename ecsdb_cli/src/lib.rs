pub mod commands;
pub use commands::{Command, CommandError, CommandResult, Commands};

pub mod pipeline;
pub use pipeline::Pipeline;

pub const COMMANDS: Commands = &[
    &commands::Info,
    &commands::SqliteExecute,
    &commands::Entities,
    &commands::Components,
    &commands::RegisteredSystems,
    &commands::QueryCommand,
];
