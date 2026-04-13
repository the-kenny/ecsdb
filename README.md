# `ecsdb`

Experiments in applying Entity-Component-System patterns to durable data storage APIs.

## Usage

```rust
use ecsdb::*;
use ecsdb::query::*;
use serde::{Serialize, Deserialize};

#[derive(Debug, Component, Serialize, Deserialize)]
struct Headline(String);

#[derive(Debug, Component, Serialize, Deserialize)]
struct Date(String);

let ecs = Ecs::open_in_memory().unwrap();
ecs.new_entity()
    .attach(Headline("My Note".into()))
    .attach(Date(chrono::Utc::now().to_rfc3339()));

ecs.new_entity().attach(Headline("My Note".into()));

for (entity, headline) in ecs.query::<(Entity, Headline), Without<Date>>().into_iter() {
    println!(
        "Entity '{}' (id={}) is missing component 'Date'",
        headline.0,
        entity.id()
    );

    entity.destroy();
}
```

## Components

A component is a singular piece of data, similar to a column in a relational
database.

They must implement `serde::Serialize`, `serde::Deserialize` and
`ecsdb::Component`, all of which can be `#[derive]`'d.

```rust
# use serde::{Serialize, Deserialize};
# use ecsdb::Component;

#[derive(Serialize, Deserialize, Component)]
pub struct Marker;

#[derive(Serialize, Deserialize, Component)]
pub struct Date(chrono::DateTime<chrono::Utc>);

#[derive(Serialize, Deserialize, Component)]
pub enum State {
    New,
    Processing,
    Finished
}
```

### Storage strategies

Components use one of three storage strategies:

- **JsonStorage** (default) — serialized as JSON text via serde. Requires
  `Serialize + Deserialize`.
- **BlobStorage** — raw bytes, stored as a SQLite BLOB. Requires
  `AsRef<[u8]> + From<Vec<u8>>`.
- **NullStorage** — marker components with no data, stored as SQL NULL. Applied
  automatically to unit structs.

```rust
# use ecsdb::Component;
# use serde::{Serialize, Deserialize};

// Default: JsonStorage
#[derive(Serialize, Deserialize, Component)]
struct Score(u64);

// Explicit blob storage
#[derive(Component)]
#[component(storage = "blob")]
struct ImageData(Vec<u8>);
# impl AsRef<[u8]> for ImageData {
#     fn as_ref(&self) -> &[u8] { &self.0 }
# }
# impl From<Vec<u8>> for ImageData {
#     fn from(v: Vec<u8>) -> Self { Self(v) }
# }

// Unit structs automatically use NullStorage
#[derive(Serialize, Deserialize, Component)]
struct Archived;
```

### Component attributes

```rust
# use ecsdb::Component;
# use serde::{Serialize, Deserialize};

// Override the component name stored in the database
#[derive(Serialize, Deserialize, Component)]
#[component(name = "app::Priority")]
struct Priority(u32);

// Recognize old names when reading (for renaming components)
#[derive(Serialize, Deserialize, Component)]
#[component(other_names = ["old::Title"])]
struct Title(String);
```

## Entities

```rust
# use ecsdb::{Component, Ecs, query::*};
# use serde::{Serialize, Deserialize};
# use ecsdb::doctests::*;

# let ecs = Ecs::open_in_memory().unwrap();

// Attach components via `Entity::attach`:
let entity = ecs.new_entity()
    .attach(State::New);

// To retrieve an attached component, use `Entity::component`:
let date: Option<Date> = entity.component::<Date>();

// To detach a component, use `Entity::detach`. Detaching a non-attached component is a no-op:
entity.detach::<Date>();

// Re-attaching a component of the same type overwrites the old. Attaching the
// same value is a no-op:
entity.attach(State::Finished);
```

Additional entity operations:

```rust
# use ecsdb::{Component, Ecs, query::*};
# use serde::{Serialize, Deserialize};
# use ecsdb::doctests::*;
# let ecs = Ecs::open_in_memory().unwrap();
# let entity = ecs.new_entity().attach(Marker);

// Check if an entity has a component (or a tuple of components):
assert!(entity.has::<Marker>());

// Check if an entity matches a query filter:
assert!(entity.matches::<With<Marker>>());

// Read-modify-write a component atomically:
# #[derive(Component, Default, Serialize, Deserialize)]
# struct Counter(u64);
# let entity = ecs.new_entity().attach(Counter(0));
entity.modify_component(|c: &mut Counter| c.0 += 1);

// Remove all user components from an entity:
# let entity = ecs.new_entity().attach(Marker);
entity.detach_all();

// Get an entity only if it exists:
let maybe: Option<_> = ecs.entity(999).or_none();
```

## Bundles

Multiple components can be attached at once using tuples or `#[derive(Bundle)]`:

```rust
# use ecsdb::{Component, Bundle, Ecs};
# use serde::{Serialize, Deserialize};
# #[derive(Serialize, Deserialize, Component)]
# struct Position(f64, f64);
# #[derive(Serialize, Deserialize, Component)]
# struct Health(u32);
# #[derive(Serialize, Deserialize, Component)]
# struct Name(String);

// Tuple bundles
# let ecs = Ecs::open_in_memory().unwrap();
let entity = ecs.new_entity()
    .attach((Position(0.0, 0.0), Health(100)));

// Struct bundles
#[derive(Bundle)]
struct Player {
    pos: Position,
    health: Health,
    name: Name,
}

let entity = ecs.new_entity().attach(Player {
    pos: Position(1.0, 2.0),
    health: Health(100),
    name: Name("Alice".into()),
});

// Detaching a bundle removes those components:
entity.detach::<(Position, Health)>();
```

Optional components in bundles attach only when `Some`:

```rust
# use ecsdb::{Component, Bundle, Ecs};
# use serde::{Serialize, Deserialize};
# #[derive(Serialize, Deserialize, Component)]
# struct Tag(String);
# #[derive(Serialize, Deserialize, Component)]
# struct Score(u64);

#[derive(Bundle)]
struct Entry {
    tag: Tag,
    score: Option<Score>,
}
# let ecs = Ecs::open_in_memory().unwrap();

// Score is not attached
let e = ecs.new_entity().attach(Entry {
    tag: Tag("x".into()),
    score: None,
});
assert!(!e.has::<Score>());
```

## Queries

### Filters

Queries take a data type and an optional filter:

```rust
# use ecsdb::{Component, Ecs, Entity, EntityId, query::*};
# use serde::{Serialize, Deserialize};
# #[derive(Serialize, Deserialize, Component)]
# struct A;
# #[derive(Serialize, Deserialize, Component)]
# struct B;
# #[derive(Serialize, Deserialize, Component)]
# struct C;
# let ecs = Ecs::open_in_memory().unwrap();

// With<C> — entity must have component C
let _: Vec<Entity> = ecs.query::<Entity, With<A>>().collect();

// Without<C> — entity must not have component C
let _: Vec<Entity> = ecs.query::<Entity, Without<A>>().collect();

// AnyOf<(C1, C2)> — entity must have at least one of the listed components
let _: Vec<Entity> = ecs.query::<Entity, AnyOf<(A, B)>>().collect();

// Or<(F1, F2)> — logical OR of multiple filters
let _: Vec<Entity> = ecs.query::<Entity, Or<(With<A>, With<B>)>>().collect();

// Tuple filters — logical AND
let _: Vec<Entity> = ecs.query::<Entity, (With<A>, Without<B>)>().collect();
```

### Filtering by value

`query_filtered` and `find` accept runtime filter values — component instances,
entity IDs, ranges, and tuples:

```rust
# use ecsdb::{Component, Ecs, Entity, EntityId, query::*};
# use serde::{Serialize, Deserialize};
# #[derive(Serialize, Deserialize, Component, PartialEq, Debug)]
# struct Score(u64);
# let ecs = Ecs::open_in_memory().unwrap();
# let _ = ecs.new_entity().attach(Score(50));
# let _ = ecs.new_entity().attach(Score(150));

// Find entities with an exact component value
let results: Vec<_> = ecs.query_filtered::<Entity, ()>(Score(50)).collect();

// Range queries
let results: Vec<_> = ecs
    .query_filtered::<Entity, ()>(Score(0)..Score(100))
    .collect();

// Open-ended ranges
let high: Vec<_> = ecs.query_filtered::<Entity, ()>(Score(100)..).collect();
let low: Vec<_> = ecs.query_filtered::<Entity, ()>(..Score(100)).collect();

// find() is shorthand for query_filtered::<Entity, ()>
let results: Vec<_> = ecs.find(Score(50)).collect();
```

## Resources

Resources are singleton components stored on the world entity (ID 0):

```rust
# use ecsdb::{Component, Ecs};
# use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Component, Default)]
struct Config { max_retries: u32 }

let mut ecs = Ecs::open_in_memory().unwrap();

ecs.attach_resource(Config { max_retries: 3 });
let config = ecs.resource::<Config>().unwrap();
assert_eq!(config.max_retries, 3);

// resource_mut returns a proxy that auto-saves on drop
{
    let mut config = ecs.resource_mut::<Config>();
    config.max_retries = 5;
}

ecs.detach_resource::<Config>();
```

## Systems

Systems are functions operating on an `Ecs`. They can be run via
`Ecs::run_system`. They take injectable parameters to access data in the `Ecs`:

```rust
# use ecsdb::doctests::*;
use ecsdb::query::{Query, With, Without};

// This system will attach `State::New` to all entities that have a `Marker` but
// no `State` component
fn process_marked_system(marked_entities: Query<Entity, (With<Marker>, Without<State>)>) {
    for entity in marked_entities.iter() {
        entity
            .attach(State::New)
            .detach::<Marker>();
    }
}

// This system logs all entities that have both `Date` and `Marker` but no
// `State`
fn log_system(entities: Query<(EntityId, Date, Marker), Without<State>>) {
    for (entity_id, Date(date), _marker) in entities.iter() {
        println!("{entity_id} {date}");
    }
}

let ecs = Ecs::open_in_memory().unwrap();
ecs.run_system(process_marked_system).unwrap();
ecs.run_system(log_system).unwrap();
```

### System parameters

System functions can accept any combination of these injectable parameters:

- `&Ecs` — direct access to the database
- `Query<D, F>` — a query over entities
- `SystemEntity<'_>` — the system's own entity (for storing per-system state)
- `LastRun` — timestamp of the system's last execution
- `&E` where `E: Extension` — custom data registered with `Ecs::register_extension`

Systems can return `()` or `Result<(), anyhow::Error>`.

## Extensions

Extensions let you inject custom data into systems:

```rust
# use ecsdb::{Ecs, Extension};

struct ApiClient { base_url: String }
impl Extension for ApiClient {}

let mut ecs = Ecs::open_in_memory().unwrap();
ecs.register_extension(ApiClient {
    base_url: "https://api.example.com".into(),
}).unwrap();

fn sync_system(client: &ApiClient) {
    println!("Syncing from {}", client.base_url);
}

ecs.run_system(sync_system).unwrap();
```

## Dynamic components

`DynComponent` allows working with components without knowing their type at
compile time:

```rust
# use ecsdb::{Ecs, Component, DynComponent};
# use serde::{Serialize, Deserialize};
# #[derive(Serialize, Deserialize, Component)]
# struct Score(u64);
# let ecs = Ecs::open_in_memory().unwrap();
# let entity = ecs.new_entity().attach(Score(42));

// Read a component by name
if let Some(dyn_comp) = entity.dyn_component("my_app::Score") {
    match dyn_comp.kind() {
        ecsdb::dyn_component::Kind::Json => {
            let value = dyn_comp.as_json().unwrap();
            println!("{value}");
        }
        ecsdb::dyn_component::Kind::Blob => {
            let bytes = dyn_comp.as_blob().unwrap();
        }
        ecsdb::dyn_component::Kind::Null => { /* marker */ }
        _ => {}
    }
}

// List all component names on an entity
for name in entity.component_names() {
    println!("{name}");
}
```

## Scheduling

`ecsdb::Schedule` allows scheduling of different systems by different criterias:

```rust
# use ecsdb::doctests::*;
# let ecs  = Ecs::open_in_memory().unwrap();

fn sys_a() {}
fn sys_b() {}

use ecsdb::schedule::*;
let mut schedule = Schedule::new();

// Run `sys_a` every 15 minutes
schedule.add(sys_a, Every(chrono::Duration::minutes(15)));

// Run `sys_b` after `sys_a`
schedule.add(sys_b, After::system(sys_a));

// Run all pending systems
schedule.tick(&ecs);
```

- `schedule::Every(Duration)` runs a system periodically
- `schedule::After` runs one system after another finished
- `schedule::Once` runs a system once per database
- `schedule::Always` runs a system on every `Schedule::tick`
- `schedule::Manually` registers a system but never auto-runs it; invoke with
  `schedule.run_system(&ecs, name)`

Systems can also be enabled/disabled at runtime via `schedule.enable(sys)` /
`schedule.disable(sys)`.

## Database

`ecsdb` uses a single SQLite database with one table:

```sql
components(entity INT, component TEXT, data BLOB)
```

- **WAL mode** is enabled automatically for concurrent read performance.
- **`CreatedAt` and `LastUpdated`** timestamps are managed by SQLite triggers,
  not application code. Every entity automatically tracks when it was created
  and last modified.
- **Migrations** run automatically on `Ecs::open()`.
- **Direct SQL access** is available via `ecs.raw_sql()` for custom queries
  against the underlying `rusqlite::Connection`.

## Web UI

`ecsdb_web` provides a web interface built on Axum + Maud + htmx:

```rust,ignore
let service = ecsdb_web::service("/db", move |_req| {
    ecsdb::Ecs::open("my.db")
});
```

The web UI supports browsing entities, filtering by component names, viewing and
editing component data (JSON and blob), and deleting components.

## CLI

`ecsdb_cli` provides an `ecsdb` binary with an interactive REPL:

```sh
ecsdb my.db 'query all | filter(component == "foo::bar::Headline") | take(10)'
```

The `query` command takes a pipeline of stages separated by `|`. Available
stages include `all`, `filter(expr)`, `sortBy(field [asc|desc])`, `take(n)`, and
`skip(n)`. 

Filter expressions compare a column from `{entity, component, data}` against a
value using `==`, `=`, `!=`, `<`, `<=`, `>`, `>=`.

### Filter values

The right hand side of a comparison in `filter(...)` expressions are parsed as
JSON literals:

```text
query all | filter(data == null)
query all | filter(entity == -1)
query all | filter(data == 1.5e2)
query all | filter(data == "hello\nworld")
query all | filter(data == [1, 2, 3])
query all | filter(data == {"key": "value"})
query all | filter(data == [{"a": 1}, {"a": 2}])
```

### Path access on `data`

JSON values in `data` can be accessed with a simplified JSON access notation:

```text
query all | filter(data.name == "Foo")
query all | filter(data.items[0].id == "x")
query all | filter(data.a.b.c == null)
query all | sortBy(data.priority desc)
```
