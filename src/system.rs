use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use crate::{self as ecsdb, query, Component, Ecs, Entity};

use core::marker::PhantomData;
use std::borrow::{Borrow, Cow};

#[derive(Serialize, Deserialize, Component, Debug, PartialEq, Eq, Hash)]
pub struct Name(pub String);

#[derive(Serialize, Deserialize, Component, Debug)]
pub struct LastRun(pub chrono::DateTime<chrono::Utc>);

pub trait System: 'static + Send {
    fn name(&self) -> Cow<'static, str>;
    fn run(&self, app: &Ecs) -> Result<(), anyhow::Error>;
}

pub trait IntoSystem<Params> {
    type System: System;

    fn into_system(self) -> Self::System;
}

impl<F, Params: SystemParam + 'static> IntoSystem<Params> for F
where
    F: SystemParamFunction<Params>,
{
    type System = FunctionSystem<F, Params>;

    fn into_system(self) -> Self::System {
        FunctionSystem {
            system: self,
            params: PhantomData,
        }
    }
}

pub struct FunctionSystem<F: 'static, Params: SystemParam> {
    system: F,
    params: PhantomData<fn() -> Params>,
}

impl<F, Params: SystemParam + 'static> System for FunctionSystem<F, Params>
where
    F: SystemParamFunction<Params>,
{
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed(std::any::type_name::<F>())
    }

    fn run(&self, app: &Ecs) -> Result<(), anyhow::Error> {
        SystemParamFunction::run(&self.system, F::Param::get_param(app, &self.name())).into_result()
    }
}

trait SystemParamFunction<Marker>: Send + Sync + 'static {
    type Param: SystemParam;
    fn run(&self, param: <Self::Param as SystemParam>::Item<'_>) -> Result<(), anyhow::Error>;
}

pub trait SystemOutput {
    fn into_result(self) -> Result<(), anyhow::Error>;
}

impl SystemOutput for () {
    fn into_result(self) -> Result<(), anyhow::Error> {
        Ok(())
    }
}

impl SystemOutput for Result<(), anyhow::Error> {
    fn into_result(self) -> Result<(), anyhow::Error> {
        self
    }
}

impl<F, Out> SystemParamFunction<()> for F
where
    F: Fn() -> Out + Send + Sync + 'static,
    Out: SystemOutput,
{
    type Param = ();
    fn run(&self, _app: ()) -> Result<(), anyhow::Error> {
        self().into_result()
    }
}

type SystemParamItem<'world, P> = <P as SystemParam>::Item<'world>;

macro_rules! impl_system_function {
    ($($param: ident),*) => {
        impl<F, Out, $($param: SystemParam),*> SystemParamFunction<($($param,)*)> for F
        where
            F: Send + Sync + 'static,
            for<'a> &'a F:
                Fn($($param),*) -> Out
                +
                Fn($(SystemParamItem<$param>),*) -> Out,
            Out: SystemOutput,
        {
            type Param = ($($param,)*);

            #[allow(non_snake_case)]
            #[allow(clippy::too_many_arguments)]
            fn run(&self, p: SystemParamItem<($($param,)*)>) -> Result<(), anyhow::Error> {
                let ($($param,)*) = p;
                (&self)( $($param),*).into_result()
            }
        }

        impl<$($param: SystemParam,)*> SystemParam for ($($param,)*) {
            type Item<'world> = ($($param::Item<'world>,)*);

            fn get_param<'world>(world: &'world Ecs, system: &str) -> Self::Item<'world> {
                ($($param::get_param(world, system),)*)
            }
        }
    };
}

impl_system_function!(P1);
impl_system_function!(P1, P2);
impl_system_function!(P1, P2, P3);
impl_system_function!(P1, P2, P3, P4);
impl_system_function!(P1, P2, P3, P4, P5);
impl_system_function!(P1, P2, P3, P4, P5, P6);
impl_system_function!(P1, P2, P3, P4, P5, P6, P7);

pub trait SystemParam: Sized {
    type Item<'world>: SystemParam;
    fn get_param<'world>(world: &'world Ecs, system: &str) -> Self::Item<'world>;
}

impl SystemParam for () {
    type Item<'world> = ();

    fn get_param<'world>(_world: &'world Ecs, _system: &str) -> Self::Item<'world> {
        ()
    }
}

impl Ecs {
    pub fn run<F: IntoSystem<Params>, Params: SystemParam>(
        &self,
        system: F,
    ) -> Result<(), anyhow::Error> {
        let system = system.into_system();

        let _span = tracing::info_span!("system", name = system.name().as_ref()).entered();

        let started = std::time::Instant::now();

        let system_entity = self
            .system_entity(&system.name())
            .unwrap_or_else(|| self.new_entity().attach(Name(system.name().to_string())));

        debug!("Running");

        if let Err(e) = system.run(&self) {
            error!(?e);
            return Err(e);
        }

        system_entity.attach(LastRun(chrono::Utc::now()));

        debug!(elapsed_ms = started.elapsed().as_millis(), "Finished",);

        Ok(())
    }

    pub fn system_entities<'a>(&'a self) -> impl Iterator<Item = (String, Entity<'a>)> {
        self.query::<(Entity, Name)>().map(|(e, name)| (name.0, e))
    }

    pub fn system_entity<'a>(&'a self, name: &str) -> Option<Entity<'a>> {
        self.query::<(Entity, Name)>()
            .find_map(|(e, s)| (s.0 == name).then_some(e))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemEntity<'a>(pub Entity<'a>);

impl<'a> AsRef<Entity<'a>> for SystemEntity<'a> {
    fn as_ref(&self) -> &Entity<'a> {
        &self.0
    }
}

impl SystemParam for SystemEntity<'_> {
    type Item<'world> = SystemEntity<'world>;

    fn get_param<'world>(world: &'world Ecs, system: &str) -> Self::Item<'world> {
        let Some(entity) = world.system_entity(system) else {
            panic!("Couldn't find SystemEntity for {system:?}. This should not happen.");
        };

        SystemEntity(entity)
    }
}

impl SystemParam for &'_ Ecs {
    type Item<'world> = &'world Ecs;

    fn get_param<'world>(world: &'world Ecs, _system: &str) -> Self::Item<'world> {
        world
    }
}

impl<D, F> SystemParam for query::Query<'_, D, F>
where
    F: query::QueryFilter + Default,
{
    type Item<'world> = query::Query<'world, D, F>;

    fn get_param<'world>(world: &'world Ecs, _system: &str) -> Self::Item<'world> {
        query::Query::new(world, F::default())
    }
}

impl SystemParam for LastRun {
    type Item<'world> = LastRun;

    fn get_param<'world>(world: &'world Ecs, system: &str) -> Self::Item<'world> {
        let never = LastRun(chrono::DateTime::<chrono::Utc>::MIN_UTC);

        world
            .system_entity(system)
            .and_then(|entity| entity.component())
            .unwrap_or(never)
    }
}

#[cfg(test)]
mod tests {
    use crate::query::With;
    use crate::{query, Ecs, Entity, SystemEntity};

    #[test]
    fn no_param() {
        let ecs = Ecs::open_in_memory().unwrap();
        ecs.run(|| ()).unwrap();
    }

    #[test]
    fn ecs_param() {
        let ecs = Ecs::open_in_memory().unwrap();
        ecs.run(|_ecs: &Ecs| ()).unwrap();
        // ecs.run(|_ecs: &Ecs| ());
    }

    #[test]
    fn query_param() {
        let ecs = Ecs::open_in_memory().unwrap();
        ecs.run(|_q: query::Query<()>| ()).unwrap();
    }

    #[test]
    fn multiple_params() {
        let ecs = Ecs::open_in_memory().unwrap();
        ecs.run(|_ecs: &Ecs, _q: query::Query<()>| ()).unwrap();
        ecs.run(|_: &Ecs, _: &Ecs| ()).unwrap();
        ecs.run(|_: &Ecs, _: &Ecs, _: &Ecs| ()).unwrap();
        ecs.run(|_: &Ecs, _: &Ecs, _: &Ecs, _: &Ecs| ()).unwrap();
        ecs.run(|_: &Ecs, _: &Ecs, _: &Ecs, _: &Ecs, _: &Ecs| ())
            .unwrap();
    }

    use crate as ecsdb;
    use ecsdb::Component;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct A;

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct B;

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct Seen;

    #[test]
    fn run_query() {
        let db = Ecs::open_in_memory().unwrap();
        fn system(query: query::Query<Entity, With<(A, B)>>) {
            for entity in query.try_iter().unwrap() {
                entity.attach(Seen);
            }
        }

        // db.register(system);

        let a_and_b = db.new_entity().attach(A).attach(B);
        let a = db.new_entity().attach(A);

        db.run(system).unwrap();

        assert!(a_and_b.component::<Seen>().is_some());
        assert!(a.component::<Seen>().is_none());
    }

    #[test]
    fn run_ecs() {
        let db = Ecs::open_in_memory().unwrap();
        fn system(ecs: &Ecs) {
            ecs.new_entity().attach(Seen);
        }

        db.run(system).unwrap();

        assert!(db.query::<Seen>().next().is_some());
    }

    #[test]
    fn system_entity_param() {
        let db = Ecs::open_in_memory().unwrap();
        fn system(ecs: &Ecs, system: SystemEntity<'_>) {
            assert_eq!(
                system
                    .as_ref()
                    .component::<crate::system::Name>()
                    .unwrap()
                    .0,
                "ecsdb::system::tests::system_entity_param::system"
            );

            ecs.new_entity().attach(Seen);
        }

        db.run(system).unwrap();

        assert!(db.query::<Seen>().next().is_some());
    }
}

impl AsRef<chrono::DateTime<chrono::Utc>> for LastRun {
    fn as_ref(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.0
    }
}

impl Borrow<chrono::DateTime<chrono::Utc>> for LastRun {
    fn borrow(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.0
    }
}
