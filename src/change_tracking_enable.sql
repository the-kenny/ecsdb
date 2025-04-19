-- Runs when a component is attached
create trigger if not exists components_changes_attach_trigger 
after insert 
on components for each row 
begin
    insert into changes (entity, component, change)
        select new.entity, new.component, 'create'
        where not exists (
            select true from components 
                where entity = new.entity 
                and component != new.component
        );
        
    insert into changes (entity, component, change)
        select new.entity, new.component, 'attach';
end;

-- Runs when a component is changed
create trigger if not exists components_changes_update_trigger 
after update 
on components for each row 
begin
    insert into changes (entity, component, change)
        values (new.entity, new.component, 'attach');
end;

-- Runs when a component is detached
create trigger if not exists components_changes_detach_trigger 
after delete 
on components for each row 
begin
    insert into changes (entity, component, change)
        values (old.entity, old.component, 'detach');
        
    insert into changes (entity, component, change)
        select old.entity, old.component, 'destroy'
        where not exists (
            select true from components where entity = old.entity 
        );
end;
