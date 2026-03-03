-- Components
create table if not exists components (
    entity integer not null,
    component text not null,
    data blob
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

create view if not exists system_components (component) as
values ('ecsdb::CreatedAt'), ('ecsdb::LastUpdated');

-- Set ecsdb::CreatedAt on initial insert
create trigger if not exists components_created_insert_trigger
after insert on components
for each row
when new.component != 'ecsdb::CreatedAt' begin
insert into
    components (entity, component, data)
values
    (
        new.entity,
        'ecsdb::CreatedAt',
        json_quote (strftime ('%Y-%m-%dT%H:%M:%fZ'))
    ) on conflict do nothing;
end;

-- Update ecsdb::LastUpdated on update
create trigger if not exists components_last_modified_update_trigger after
update on components for each row when new.component != 'ecsdb::LastUpdated' begin
insert into
    components (entity, component, data)
values
    (
        new.entity,
        'ecsdb::LastUpdated',
        json_quote (strftime ('%Y-%m-%dT%H:%M:%fZ'))
    ) on conflict (entity, component) do
update
set
    data = excluded.data;
end;

-- Update ecsdb::LastUpdated on insert
create trigger if not exists components_last_modified_insert_trigger
after insert on components
for each row when new.component != 'ecsdb::LastUpdated'
begin
    insert into
        components (entity, component, data)
    values
        (
            new.entity,
            'ecsdb::LastUpdated',
            json_quote (strftime ('%Y-%m-%dT%H:%M:%fZ'))
        ) on conflict (entity, component) do
    update
    set
        data = excluded.data;
end;

-- Update ecsdb::LastUpdated on delete, but only when non-system components remain

drop trigger if exists components_last_modified_delete_trigger;
drop trigger if exists components_last_modified_delete_trigger_v2;

create trigger if not exists components_last_modified_delete_trigger_v3
after delete on components
for each row when old.component not in (select component from system_components) and exists (
    select
        1
    from
        components
    where
        entity = old.entity
        and component not in (select component from system_components)
)
begin
    insert into
        components (entity, component, data)
    values
        (
            old.entity,
            'ecsdb::LastUpdated',
            json_quote (strftime ('%Y-%m-%dT%H:%M:%fZ'))
        ) on conflict (entity, component) do
    update
    set
        data = excluded.data;
end;

-- Delete system components when only system components remain

drop trigger if exists components_last_modified_delete_last_component_trigger;
drop trigger if exists components_last_modified_delete_last_component_trigger_v2;

create trigger if not exists components_last_modified_delete_last_component_trigger_v3
after delete on components
for each row when old.component not in (select component from system_components) and not exists (
    select
        1
    from
        components
    where
        entity = old.entity
        and component not in (select component from system_components)
)
begin
    delete from components
    where
        entity = old.entity
        and component in (select component from system_components);
end;

create view if not exists empty_entities (entity) as
select
    entity
from
    components
group by
    entity
having
    count(*) filter (
        where component not in (select component from system_components)
    ) = 0;

-- Resources
create table if not exists resources (
    name text not null unique,
    data blob,
    last_modified rfc3339 not null default (strftime ('%Y-%m-%dT%H:%M:%fZ'))
);

create trigger if not exists resources_last_modified_trigger before
update on resources for each row
begin
    update resources
    set
        last_modified = strftime ('%Y-%m-%dT%H:%M:%fZ')
    where
        name = new.name;
end;
