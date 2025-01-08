create table if not exists components (
    entity integer not null,
    component text not null,
    data blob
);

create unique index if not exists components_entity_component_unqiue_idx on components (entity, component);

create index if not exists components_component_idx on components (component);

create table if not exists resources (
  resource text not null unique,
  data blob
);