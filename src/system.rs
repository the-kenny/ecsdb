use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::{self as ecsdb, query, Component, Ecs, Entity};

use core::marker::PhantomData;
use std::{
    borrow::{Borrow, Cow},
    ops::Deref,
};

#[derive(Serialize, Deserialize, Component, Debug, PartialEq, Eq, Hash)]
pub struct Name(pub String);

#[derive(Serialize, Deserialize, Component, Debug)]
pub struct LastRun(pub chrono::DateTime<chrono::Utc>);

pub trait System: Send + Sync {
    fn name(&self) -> Cow<'static, str>;
    fn run_system(&self, app: &Ecs) -> Result<(), anyhow::Error>;
}

pub trait IntoSystem<Marker>: Sized {
    type System: System;
    fn into_system(self) -> Self::System;

    fn into_boxed_system(self) -> BoxedSystem
    where
        Self::System: 'static,
    {
        Box::new(self.into_system())
    }
}

impl<S: System> IntoSystem<()> for S {
    type System = S;

    fn into_system(self) -> Self::System {
        self
    }
}

impl<'a, S: System> System for &'a S {
    fn name(&self) -> Cow<'static, str> {
        (*self).name()
    }

    fn run_system(&self, app: &Ecs) -> Result<(), anyhow::Error> {
        (*self).run_system(app)
    }
}

pub type BoxedSystem = Box<dyn System>;

impl System for BoxedSystem {
    fn name(&self) -> Cow<'static, str> {
        System::name(self.as_ref())
    }

    fn run_system(&self, app: &Ecs) -> Result<(), anyhow::Error> {
        System::run_system(self.as_ref(), app)
    }
}

#[doc(hidden)]
pub struct FunctionSystemMarker;

impl<Marker, F> IntoSystem<(Marker, FunctionSystemMarker)> for F
where
    Marker: 'static,
    F: SystemParamFunction<Marker>,
{
    type System = FunctionSystem<Marker, F>;

    fn into_system(self) -> Self::System {
        FunctionSystem {
            system: self,
            params: PhantomData,
        }
    }
}

pub struct FunctionSystem<Marker, F>
where
    F: 'static,
{
    system: F,
    params: PhantomData<fn() -> Marker>,
}

impl<Marker, F> System for FunctionSystem<Marker, F>
where
    Marker: 'static,
    F: SystemParamFunction<Marker>,
{
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed(std::any::type_name::<F>())
    }

    fn run_system(&self, app: &Ecs) -> Result<(), anyhow::Error> {
        SystemParamFunction::run_system(&self.system, F::Params::get_param(app, &self.name()))
            .into_result()
    }
}

pub trait SystemParamFunction<Marker>: Send + Sync + 'static {
    type Params: SystemParam;
    fn run_system(
        &self,
        param: <Self::Params as SystemParam>::Item<'_>,
    ) -> Result<(), anyhow::Error>;
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
    type Params = ();
    fn run_system(&self, _app: ()) -> Result<(), anyhow::Error> {
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
            type Params = ($($param,)*);

            #[allow(non_snake_case)]
            #[allow(clippy::too_many_arguments)]
            fn run_system(&self, p: SystemParamItem<($($param,)*)>) -> Result<(), anyhow::Error> {
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
    #[deprecated(note = "use Ecs::run_system")]
    pub fn run<Marker, F: IntoSystem<Marker>>(&self, system: F) -> Result<(), anyhow::Error> {
        self.run_system(system)
    }

    pub fn run_system<'a, Marker, F: IntoSystem<Marker> + 'a>(
        &'a self,
        system: F,
    ) -> Result<(), anyhow::Error> {
        let system = system.into_system();
        self.run_dyn_system(&system)
    }

    #[instrument(level="info", name="run_system", skip_all, fields(name = %system.name()))]
    pub(crate) fn run_dyn_system(&self, system: &dyn System) -> Result<(), anyhow::Error> {
        let started = std::time::Instant::now();

        let system_entity = self.get_or_create_system_entity(&system.name());

        info!("Running");

        if let Err(e) = system.run_system(&self) {
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
    pub(crate) fn get_or_create_system_entity<'a>(&'a self, system: &str) -> Entity<'a> {
        self.system_entity(&system)
            .unwrap_or_else(|| self.new_entity().attach(Name(system.to_string())))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemEntity<'a>(pub Entity<'a>);

impl<'a> AsRef<Entity<'a>> for SystemEntity<'a> {
    fn as_ref(&self) -> &Entity<'a> {
        &self.0
    }
}

impl<'a> Deref for SystemEntity<'a> {
    type Target = Entity<'a>;

    fn deref(&self) -> &Self::Target {
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
        query::Query::new(world)
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

#[cfg(test)]
mod tests {
    use std::marker::PhantomData;

    use crate::query::With;
    use crate::{query, Ecs, Entity, IntoSystem, System, SystemEntity};

    #[test]
    fn run_system() {
        let ecs = Ecs::open_in_memory().unwrap();
        ecs.run_system(|| ()).unwrap();
    }

    #[test]
    fn run_system_boxed() {
        let ecs = Ecs::open_in_memory().unwrap();
        let system = IntoSystem::into_boxed_system(|| ());
        ecs.run_system(&system).unwrap();
        ecs.run_system(system).unwrap();
    }

    #[test]
    fn run_dyn_system() {
        let ecs = Ecs::open_in_memory().unwrap();
        let system = IntoSystem::into_boxed_system(|| ());
        ecs.run_dyn_system(&system).unwrap();
        ecs.run_dyn_system(system.as_ref()).unwrap();
    }

    #[test]
    fn non_static_system() {
        let ecs = Ecs::open_in_memory().unwrap();

        struct NonStaticSystem<'a>(PhantomData<&'a ()>);
        #[rustfmt::skip]
        impl<'a> System for NonStaticSystem<'a> {
            fn name(&self) -> std::borrow::Cow<'static, str> { "".into() }
            fn run_system(&self, _app: &Ecs) -> Result<(), anyhow::Error> { Ok(()) }
        }

        let non_static: NonStaticSystem<'_> = NonStaticSystem(PhantomData);
        ecs.run_system(&non_static).unwrap();
    }

    #[test]
    fn no_param() {
        let ecs = Ecs::open_in_memory().unwrap();
        ecs.run_system(|| ()).unwrap();
    }

    #[test]
    fn ecs_param() {
        let ecs = Ecs::open_in_memory().unwrap();
        ecs.run_system(|_ecs: &Ecs| ()).unwrap();
        // ecs.run_system(|_ecs: &Ecs| ());
    }

    #[test]
    fn query_param() {
        let ecs = Ecs::open_in_memory().unwrap();
        ecs.run_system(|_q: query::Query<()>| ()).unwrap();
    }

    #[test]
    fn multiple_params() {
        let ecs = Ecs::open_in_memory().unwrap();
        ecs.run_system(|_ecs: &Ecs, _q: query::Query<()>| ())
            .unwrap();
        ecs.run_system(|_: &Ecs, _: &Ecs| ()).unwrap();
        ecs.run_system(|_: &Ecs, _: &Ecs, _: &Ecs| ()).unwrap();
        ecs.run_system(|_: &Ecs, _: &Ecs, _: &Ecs, _: &Ecs| ())
            .unwrap();
        ecs.run_system(|_: &Ecs, _: &Ecs, _: &Ecs, _: &Ecs, _: &Ecs| ())
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
    fn run_query_param() {
        let db = Ecs::open_in_memory().unwrap();
        fn system(query: query::Query<Entity, With<(A, B)>>) {
            for entity in query.try_iter().unwrap() {
                entity.attach(Seen);
            }
        }

        // db.register(system);

        let a_and_b = db.new_entity().attach(A).attach(B);
        let a = db.new_entity().attach(A);

        db.run_system(system).unwrap();

        assert!(a_and_b.component::<Seen>().is_some());
        assert!(a.component::<Seen>().is_none());
    }

    #[test]
    fn run_ecs_param() {
        let db = Ecs::open_in_memory().unwrap();
        fn system(ecs: &Ecs) {
            ecs.new_entity().attach(Seen);
        }

        db.run_system(system).unwrap();

        assert!(db.query::<Seen>().next().is_some());
    }

    #[test]
    fn run_system_entity_param() {
        let db = Ecs::open_in_memory().unwrap();
        fn system(ecs: &Ecs, system: SystemEntity<'_>) {
            assert_eq!(
                system.component::<crate::system::Name>().unwrap().0,
                "ecsdb::system::tests::run_system_entity_param::system"
            );

            ecs.new_entity().attach(Seen);
        }

        db.run_system(system).unwrap();

        assert!(db.query::<Seen>().next().is_some());
    }
}
