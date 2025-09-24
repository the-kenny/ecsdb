use ecsdb::{Component, Entity, EntityId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Component)]
struct DiaryEntry;
#[derive(Debug, Serialize, Deserialize, Component)]
struct Contents(String);
#[derive(Debug, Serialize, Deserialize, Component)]
struct Date(chrono::NaiveDate);

pub fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt::init();

    let db = ecsdb::Ecs::open("basic.sqlite")?;

    let _entry = db
        .new_entity()
        .attach(DiaryEntry)
        .attach(Contents("Lorem ipsum ...".into()))
        .attach(Date(chrono::Utc::now().date_naive()));

    use ecsdb::query::*;

    println!("Total: {} entities", db.query::<EntityId, ()>().count());

    let _ = db.query::<Entity, (
        With<(DiaryEntry, Contents)>,
        Without<Date>,
        Or<(With<DiaryEntry>, With<Contents>)>,
    )>();

    for (id, _, Date(date), Contents(contents)) in
        db.query::<(EntityId, DiaryEntry, Date, Contents), ()>()
    {
        println!("DiaryEntry",);
        println!("  id:\t{}", id);
        println!("  date:\t{date}",);
        println!("  text:\t{contents}");
        println!()
    }

    db.close().unwrap();

    Ok(())
}
