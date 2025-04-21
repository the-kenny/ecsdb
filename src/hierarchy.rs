// use std::iter;

// use ecsdb_derive::Component;
// use serde::{Deserialize, Serialize};

// use crate::{self as ecsdb, Ecs, Entity, EntityId};

// #[derive(Component, Clone, Copy, Debug, Serialize, Deserialize)]
// pub struct BelongsTo(pub EntityId);

// impl Ecs {
//     pub fn direct_children<'a>(
//         &'a self,
//         entity: EntityId,
//     ) -> impl Iterator<Item = Entity<'a>> + 'a {
//         self.find(BelongsTo(entity))
//     }

//     pub fn all_children<'a>(&'a self, entity: EntityId) -> impl Iterator<Item = Entity<'a>> + 'a {
//         let mut stack = self.direct_children(entity).collect::<Vec<_>>();
//         iter::from_fn(move || -> Option<Entity<'a>> {
//             let Some(entity) = stack.pop() else {
//                 return None;
//             };

//             for entity in self.direct_children(entity.id()) {
//                 stack.push(entity);
//             }

//             Some(entity)
//         })
//     }
// }

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn belongs_to() {
//         #[derive(Debug, Serialize, Deserialize, Component)]
//         struct A;

//         #[derive(Debug, Serialize, Deserialize, PartialEq, Component)]
//         struct B;

//         let db = Ecs::open_in_memory().unwrap();

//         let parent = db.new_entity().attach(A);
//         let child1 = db.new_entity().attach(A).attach(BelongsTo(parent.id()));
//         let child2 = db.new_entity().attach(A).attach(BelongsTo(child1.id()));

//         assert_eq!(
//             parent.direct_children().map(|e| e.id()).collect::<Vec<_>>(),
//             vec![child1.id()]
//         );

//         assert_eq!(
//             parent.all_children().map(|e| e.id()).collect::<Vec<_>>(),
//             vec![child1.id(), child2.id()]
//         );

//         assert_eq!(
//             child1.all_children().map(|e| e.id()).collect::<Vec<_>>(),
//             vec![child2.id()]
//         );

//         assert!(child2.all_children().next().is_none());
//     }
// }
