use std::collections::HashSet;

use ecsdb::Component;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Component)]
struct N(u64);

pub fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt::init();

    let db = ecsdb::Ecs::open("stress.sqlite")?;

    let start = std::time::Instant::now();
    for n in 0..100000 {
        db.new_entity().attach(N(n));
    }
    println!("Elapsed: {}ms", start.elapsed().as_millis());

    Ok(())
}
