use ecsdb::{query::Without, Component, Ecs};
use serde::{Deserialize, Serialize};

pub fn main() -> Result<(), anyhow::Error> {
    #[derive(Debug, Component, Serialize, Deserialize)]
    struct Headline(String);

    #[derive(Debug, Component, Serialize, Deserialize)]
    struct Date(chrono::DateTime<chrono::Utc>);

    let ecs = Ecs::open_in_memory()?;
    ecs.new_entity()
        .attach(Headline("My Note".into()))
        .attach(Date(chrono::Utc::now()));

    ecs.new_entity().attach(Headline("My Note".into()));

    for entity in ecs.query::<(Headline, Without<Date>)>().into_iter() {
        println!(
            "Entity '{}' (id={}) is missing component 'Date'",
            entity.component::<Headline>().unwrap().0,
            entity.id()
        );

        entity.destroy();
    }

    Ok(())
}
