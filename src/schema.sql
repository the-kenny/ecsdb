-- Components
create table if not exists components (
    entity integer not null,
    component text not null,
    data blob,
    last_modified rfc3339 not null default (strftime ('%Y-%m-%dT%H:%M:%fZ'))
);

create unique index if not exists components_entity_component_unqiue_idx on components (entity, component);

create index if not exists components_component_idx on components (component);

create view if not exists entity_components (entity, components) as
select
    entity,
    json_group_array (component)
from
    components
group by
    entity
order by
    component asc;

create trigger if not exists components_last_modified_trigger before
update on components for each row begin
update components
set
    last_modified = strftime ('%Y-%m-%dT%H:%M:%fZ')
where
    entity = new.entity
    and component = new.component;

end;

create table if not exists resources (
    name text not null unique,
    data blob,
    last_modified rfc3339 not null default (strftime ('%Y-%m-%dT%H:%M:%fZ'))
);

create trigger if not exists resources_last_modified_trigger before
update on resources for each row begin
update resources
set
    last_modified = strftime ('%Y-%m-%dT%H:%M:%fZ')
where
    name = new.name;

end;
