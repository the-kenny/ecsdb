begin;

insert or replace into components (entity, component, data)
select 0, name, data from resources;

drop trigger if exists resources_last_modified_trigger;
drop table if exists resources;

commit;
