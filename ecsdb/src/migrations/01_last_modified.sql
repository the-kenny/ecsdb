begin;

alter table components
rename to components_old;

create table components (
    entity integer not null,
    component text not null,
    data blob,
    last_modified rfc3339 not null default (strftime ('%Y-%m-%dT%H:%M:%fZ'))
);

create unique index if not exists components_entity_component_unqiue_idx on components (entity, component);

create index if not exists components_component_idx on components (component);

insert into
    components (entity, component, data)
select
    *
from
    components_old;

create trigger if not exists components_last_modified_trigger before
update on components for each row begin
update components
set
    last_modified = strftime ('%Y-%m-%dT%H:%M:%fZ')
where
    entity = new.entity
    and component = new.component;

end;

alter table resources
rename to resources_old;

create table resources (
    name text not null unique,
    data blob,
    last_modified rfc3339 not null default (strftime ('%Y-%m-%dT%H:%M:%fZ'))
);

insert into
    resources (name, data)
select
    *
from
    resources_old;

create trigger if not exists resources_last_modified_trigger before
update on resources for each row begin
update resources
set
    last_modified = strftime ('%Y-%m-%dT%H:%M:%fZ')
where
    name = new.name;

end;

drop table components_old;

drop table resources_old;

commit;
