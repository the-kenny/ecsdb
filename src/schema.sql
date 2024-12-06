create table if not exists components (
	entity integer not null,
	component text not null,
	data blob
);

create unique index if not exists components_entity_component_unqiue 
	on components (entity, component);
