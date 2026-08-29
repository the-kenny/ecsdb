use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument, warn};

use crate::{self as ecsdb, Component, Ecs, Entity, query};

use core::marker::PhantomData;
use std::{
    borrow::{Borrow, Cow},
    ops::Deref,
};

#[derive(Serialize, Deserialize, Component, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(pub String);

#[derive(Serialize, Deserialize, Component, Debug)]
pub struct LastRun(pub chrono::DateTime<chrono::Utc>);

#[derive(Serialize, Deserialize, Component, Debug)]
pub struct LastResult(pub SystemResult<(), String>);

pub trait System: Send + Sync {
    fn name(&self) -> Cow<'static, str>;
    fn run_system(&self, app: &Ecs) -> SystemResult;
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

    fn run_system(&self, app: &Ecs) -> SystemResult {
        (*self).run_system(app)
    }
}

pub type BoxedSystem = Box<dyn System>;

impl System for BoxedSystem {
    fn name(&self) -> Cow<'static, str> {
        System::name(self.as_ref())
    }

    fn run_system(&self, app: &Ecs) -> SystemResult {
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

    fn run_system(&self, app: &Ecs) -> SystemResult {
        SystemParamFunction::run_system(&self.system, F::Params::get_param(app, &self.name()))
    }
}

pub trait SystemParamFunction<Marker>: Send + Sync + 'static {
    type Params: SystemParam;
    fn run_system(&self, param: <Self::Params as SystemParam>::Item<'_>) -> SystemResult;
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub enum SystemResult<S = (), E = anyhow::Error> {
    Ok(S),
    Err(E),
    Skipped,
}

impl<S, E> From<Result<S, E>> for SystemResult<S, E> {
    fn from(value: Result<S, E>) -> Self {
        match value {
            Ok(v) => Self::Ok(v),
            Err(v) => Self::Err(v),
        }
    }
}

impl<S, E> From<Result<std::ops::ControlFlow<(), S>, E>> for SystemResult<S, E> {
    fn from(value: Result<std::ops::ControlFlow<(), S>, E>) -> Self {
        use std::ops::ControlFlow;
        match value {
            Ok(ControlFlow::Continue(v)) => Self::Ok(v),
            Ok(ControlFlow::Break(())) => Self::Skipped,
            Err(e) => Self::Err(e),
        }
    }
}

pub trait SystemOutput {
    fn into_result(self) -> SystemResult;
}

impl SystemOutput for () {
    fn into_result(self) -> SystemResult {
        SystemResult::Ok(())
    }
}

impl SystemOutput for SystemResult {
    fn into_result(self) -> SystemResult {
        self
    }
}

impl SystemOutput for Result<(), anyhow::Error> {
    fn into_result(self) -> SystemResult {
        SystemResult::from(self)
    }
}

impl<E: Into<anyhow::Error>> SystemOutput for Result<std::ops::ControlFlow<(), ()>, E> {
    fn into_result(self) -> SystemResult {
        SystemResult::from(self.map_err(Into::into))
    }
}

impl<F, Out> SystemParamFunction<()> for F
where
    F: Fn() -> Out + Send + Sync + 'static,
    Out: SystemOutput,
{
    type Params = ();
    fn run_system(&self, _app: ()) -> SystemResult {
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
            fn run_system(&self, p: SystemParamItem<($($param,)*)>) -> SystemResult {
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

    fn get_param<'world>(_world: &'world Ecs, _system: &str) -> Self::Item<'world> {}
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

        if system.name().ends_with("{{closure}}") {
            warn!("System looks like a closure. Its name may not be unique");
        }

        info!("Running");

        let result = system.run_system(self);

        debug!(elapsed_ms = started.elapsed().as_millis(), "Finished");

        match result {
            SystemResult::Ok(v) => {
                system_entity
                    .attach((LastRun(chrono::Utc::now()), LastResult(SystemResult::Ok(v))));
                Ok(())
            }
            SystemResult::Err(error) => {
                error!(%error);
                system_entity.attach((
                    LastRun(chrono::Utc::now()),
                    LastResult(SystemResult::Err(error.to_string())),
                ));
                Err(error)
            }
            // Skipped systems did no work: leave `LastRun`/`LastResult`
            // untouched so idle ticks cause no database writes. Note that
            // `Once`/`Every` scheduling therefore treats a skipped system as
            // not having run and will re-schedule it.
            SystemResult::Skipped => Ok(()),
        }
    }

    pub fn system_entities<'a>(&'a self) -> impl Iterator<Item = (String, Entity<'a>)> {
        self.query::<(Entity, Name), ()>()
            .map(|(e, name)| (name.0, e))
    }

    pub fn system_entity<'a>(&'a self, name: &str) -> Option<Entity<'a>> {
        self.query::<(Entity, Name), ()>()
            .find_map(|(e, s)| (s.0 == name).then_some(e))
    }
    pub(crate) fn get_or_create_system_entity<'a>(&'a self, system: &str) -> Entity<'a> {
        self.system_entity(system)
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

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use std::marker::PhantomData;

    use crate::query::With;
    use crate::{
        Ecs, Entity, IntoSystem, LastResult, LastRun, System, SystemEntity, SystemResult, query,
    };

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
    #[ignore = "closure names not unique"]
    fn closure_system_names() {
        let a = IntoSystem::into_boxed_system(|| Ok(()));
        let b = IntoSystem::into_boxed_system(|| Ok(()));
        assert_ne!(a.name(), b.name());
    }

    #[test]
    fn run_dyn_system_components() {
        let ecs = Ecs::open_in_memory().unwrap();
        let ok_system = IntoSystem::into_boxed_system(|| SystemResult::Ok(()));
        let err_system = IntoSystem::into_boxed_system(|| SystemResult::Err(anyhow!("whatever")));
        let skip_system = IntoSystem::into_boxed_system(|| SystemResult::Skipped);

        fn get(ecs: &Ecs, system: &str) -> (Option<LastRun>, Option<LastResult>) {
            let system = ecs.system_entity(system).unwrap();
            (system.component(), system.component())
        }

        // Note: all three closures share the same `{{closure}}` system name
        // (and thus the same system entity), so the skip case must run first.

        // Skipped systems must not write any bookkeeping components.
        let _ = ecs.run_dyn_system(&skip_system);
        let (last_run, last_result) = get(&ecs, &skip_system.name());
        assert!(last_run.is_none());
        assert!(last_result.is_none());

        let _ = ecs.run_dyn_system(&ok_system);
        let (last_run, last_result) = get(&ecs, &ok_system.name());
        assert!(last_run.is_some());
        assert!(matches!(
            last_result,
            Some(LastResult(SystemResult::Ok(())))
        ));

        let _ = ecs.run_dyn_system(&err_system);
        let (last_run, last_result) = get(&ecs, &err_system.name());
        assert!(last_run.is_some());
        assert!(matches!(
            last_result,
            Some(LastResult(SystemResult::Err(_)))
        ));
    }

    #[test]
    fn run_dyn_system_skip_preserves_previous_result() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static SKIP: AtomicBool = AtomicBool::new(false);

        struct ToggleSystem;
        impl System for ToggleSystem {
            fn name(&self) -> std::borrow::Cow<'static, str> {
                "toggle_system".into()
            }
            fn run_system(&self, _app: &Ecs) -> SystemResult {
                if SKIP.load(Ordering::SeqCst) {
                    SystemResult::Skipped
                } else {
                    SystemResult::Ok(())
                }
            }
        }

        let ecs = Ecs::open_in_memory().unwrap();

        SKIP.store(false, Ordering::SeqCst);
        ecs.run_dyn_system(&ToggleSystem).unwrap();
        let entity = ecs.system_entity("toggle_system").unwrap();
        let last_run = entity.component::<LastRun>().unwrap();
        assert!(matches!(
            entity.component::<LastResult>(),
            Some(LastResult(SystemResult::Ok(())))
        ));

        // A subsequent skipped run keeps the previous LastRun/LastResult.
        SKIP.store(true, Ordering::SeqCst);
        ecs.run_dyn_system(&ToggleSystem).unwrap();
        let entity = ecs.system_entity("toggle_system").unwrap();
        assert_eq!(entity.component::<LastRun>().unwrap().0, last_run.0);
        assert!(matches!(
            entity.component::<LastResult>(),
            Some(LastResult(SystemResult::Ok(())))
        ));
    }

    #[test]
    fn non_static_system() {
        let ecs = Ecs::open_in_memory().unwrap();

        struct NonStaticSystem<'a>(PhantomData<&'a ()>);
        #[rustfmt::skip]
        impl<'a> System for NonStaticSystem<'a> {
            fn name(&self) -> std::borrow::Cow<'static, str> { "".into() }
            fn run_system(&self, _app: &Ecs) -> SystemResult { SystemResult::Ok(()) }
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
    use anyhow::anyhow;
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

        assert!(db.query::<Seen, ()>().next().is_some());
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

        assert!(db.query::<Seen, ()>().next().is_some());
    }

    #[test]
    fn run_system_skipped_return_value() {
        let db = Ecs::open_in_memory().unwrap();
        fn system() -> SystemResult {
            SystemResult::Skipped
        }

        assert!(matches!(
            IntoSystem::into_boxed_system(system).run_system(&db),
            SystemResult::Skipped
        ));
    }

    #[test]
    fn control_flow_into_system_result() {
        use std::ops::ControlFlow;

        let cont: Result<ControlFlow<(), i32>, String> = Ok(ControlFlow::Continue(42));
        assert_eq!(
            SystemResult::<i32, String>::from(cont),
            SystemResult::Ok(42)
        );

        let brk: Result<ControlFlow<(), i32>, String> = Ok(ControlFlow::Break(()));
        assert_eq!(
            SystemResult::<i32, String>::from(brk),
            SystemResult::Skipped
        );

        let err: Result<ControlFlow<(), i32>, String> = Err("boom".to_string());
        assert_eq!(
            SystemResult::<i32, String>::from(err),
            SystemResult::Err("boom".to_string())
        );
    }
}
