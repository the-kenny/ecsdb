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
create trigger if not exists components_last_modified_insert_trigger after insert on components for each row when new.component != 'ecsdb::LastUpdated' begin
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

-- Update ecsdb::LastUpdated on delete, except when it's the last component
create trigger if not exists components_last_modified_delete_trigger after delete on components for each row when old.component != 'ecsdb::LastUpdated'
and (
    select
        true
    from
        entity_components
    where
        entity = old.entity
        and components != json_array ('ecsdb::LastUpdated')
) begin
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

-- Delete ecsdb::LastUpdated when it's the last remaining component
create trigger if not exists components_last_modified_delete_last_component_trigger after delete on components for each row when (
    select
        true
    from
        entity_components
    where
        entity = old.entity
        and components = json_array ('ecsdb::LastUpdated')
) begin
delete from components
where
    entity = old.entity
    and component = 'ecsdb::LastUpdated';
end;

-- Resources
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
