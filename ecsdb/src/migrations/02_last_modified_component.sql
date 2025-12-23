begin;

drop trigger if exists components_last_modified_trigger;

insert
or ignore into components (entity, component, data)
select
    entity,
    'ecsdb::LastUpdated',
    json_quote (max(last_modified))
from
    components
group by
    entity;

alter table components
drop last_modified;

commit;
