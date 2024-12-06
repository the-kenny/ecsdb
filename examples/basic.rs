use std::collections::HashSet;

use ecsdb::Component;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Component)]
struct DiaryEntry;
#[derive(Debug, Serialize, Deserialize, Component)]
struct Contents(String);
#[derive(Debug, Serialize, Deserialize, Component)]
struct Date(chrono::NaiveDate);

pub fn main() -> Result<(), anyhow::Error> {
    let db = ecsdb::Ecs::open("ecs.sqlite")?;

    let entry = db
        .new_entity()
        .attach(DiaryEntry)
        .attach(Contents("Lorem ipsum ...".into()))
        .attach(Date(chrono::Utc::now().date_naive()));

    use ecsdb::query::*;
    for entry in db.query::<With<And<DiaryEntry, Contents>>>() {
        println!("DiaryEntry",);
        println!("  id:\t{}", entry.entity_id(),);
        println!(
            "  date:\t{}",
            entry.component::<Date>().unwrap().0.to_string(),
        );
        println!("  text:\t{}", entry.component::<Contents>().unwrap().0);
        println!()
    }

    Ok(())
}
