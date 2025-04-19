# `ecsdb`

Experiments in applying Entity-Component-System patterns to durable data storage APIs.

## Usage

```rust
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
```

### Components

A component is a singular piece of data, similar to a column in a relational
database.

They must implement `serde::Serialize`, `serde::Deserialize` and
`ecsdb::Component`, all of which can be `#[derive]`'d.

```rust
#[derive(Serialize, Deserialize, Component)]
pub struct Marker;

#[derive(Serialize, Deserialize, Component)]
pub struct Date(DateTime<Utc>);

#[derive(Serialize, Deserialize, Component)]
pub enum State {
    New,
    Processing,
    Finished
}
```

### Entities

```rust
// Attach components via `Entity::attach`:
let entity = Ecs::new_entity()
    .attach(Marker)
    .attach(Date(Utc::now()))
    .attach(State::New);

// To retrieve an attached component, use `Entity::component`:
let date: Option<Date> = entity.component::<Date>();

// To detach a component, use `Entity::detach`:
entity.detach::<Date>();

// Re-attaching a component of the same type overwrites the old. Attaching the
// same value is a no-op.
entity.attach(State::Finished);
```

### Systems

Systems are functions operating on an `Ecs`. They can be registerd via
`Ecs::register` and run via `Ecs::tick`. They can take a set of 'magic'
parameters to access data in the `Ecs`:

```rust
// This system will attach `State::New` to all entities that have a `Marker` component and detach `Marker`
fn process_marked_system(marked_entities: Query<(Marker, Without<State>)>) {
    for entity in marked_entities.iter() {
        entity.attach(State::New).detach::<Marker>();
    }
}

// This system logs all entities that have both `Date` and `Marker` but no
// `State`
fn log_system(entities: Query<(Marker, Date, Without<State>)>) {
    for entity in entities.iter() {
        println("{}", entity.id());
    }
}

ecs.register(process_marked_system);
ecs.register(log_system);

ecs.tick();

```
