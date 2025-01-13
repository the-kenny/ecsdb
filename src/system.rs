use tracing::debug;

use crate::{query, Ecs};

use core::marker::PhantomData;

/// This is what we store
pub trait System: 'static + Send {
    fn run(&self, app: &Ecs);
}

/// Convert thing to system (to create a trait object)
pub trait IntoSystem<Params> {
    type System: System;

    fn into_system(self) -> Self::System;
}

/// Convert any function with only system params into a system
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

/// Represent a system with its params
//
// TODO: do stuff with params
pub struct FunctionSystem<F: 'static, Params: SystemParam> {
    system: F,
    params: PhantomData<fn() -> Params>,
}

/// Make our wrapper be a System
impl<F, Params: SystemParam + 'static> System for FunctionSystem<F, Params>
where
    F: SystemParamFunction<Params>,
{
    fn run(&self, app: &Ecs) {
        SystemParamFunction::run(&self.system, F::Param::get_param(app));
    }
}

/// Function with only system params
trait SystemParamFunction<Marker>: Send + Sync + 'static {
    type Param: SystemParam;
    fn run(&self, param: <Self::Param as SystemParam>::Item<'_>);
}

pub trait SystemOutput {}
impl SystemOutput for () {}
impl SystemOutput for Result<(), anyhow::Error> {}

/// unit function
impl<F, Out> SystemParamFunction<()> for F
where
    F: Fn() -> Out + Send + Sync + 'static,
{
    type Param = ();
    fn run(&self, _app: ()) {
        eprintln!("calling a function with no params");
        self();
    }
}

/// one param function
impl<F, Out, P1: SystemParam> SystemParamFunction<(P1,)> for F
where
    F: Send + Sync + 'static,
    for<'a> &'a F: Fn(P1) -> Out + Fn(<P1 as SystemParam>::Item<'_>) -> Out,
    P1: SystemParam,
    Out: SystemOutput,
{
    type Param = (P1,);
    fn run(&self, p1: <Self::Param as SystemParam>::Item<'_>) {
        // #[allow(clippy::too_many_arguments)]
        // fn call_inner<Out, P1>(f: impl Fn(P1) -> Out, p1: P1) -> Out {
        //     f(p1)
        // }
        // let (p1,) = p1;
        // call_inner(self, p1);

        (&self)(p1.0);
    }
}

pub trait SystemParam: Sized {
    type Item<'world>: SystemParam;
    fn get_param<'world>(world: &'world Ecs) -> Self::Item<'world>;
}

impl SystemParam for () {
    type Item<'world> = ();

    fn get_param<'world>(_world: &'world Ecs) -> Self::Item<'world> {
        ()
    }
}

impl<T1> SystemParam for (T1,)
where
    T1: SystemParam,
{
    type Item<'world> = (T1::Item<'world>,);

    fn get_param<'world>(world: &'world Ecs) -> Self::Item<'world> {
        (T1::get_param(world),)
    }
}

impl Ecs {
    pub fn tick(&self) {
        for system in &self.systems {
            // let _span = tracing::info_span!("system", name = system.name().as_ref()).entered();
            let started = std::time::Instant::now();
            debug!("Running");
            system.run(&self);
            debug!(elapsed_ms = started.elapsed().as_millis(), "Finished",);
        }
    }

    pub fn register<F: IntoSystem<Params>, Params: SystemParam>(&mut self, system: F) {
        self.systems.push(Box::new(system.into_system()));
    }
}

impl SystemParam for &'_ Ecs {
    type Item<'world> = &'world Ecs;

    fn get_param<'world>(world: &'world Ecs) -> Self::Item<'world> {
        world
    }
}

impl<F> SystemParam for query::Query<'_, F, ()> {
    type Item<'world> = query::Query<'world, F, ()>;

    fn get_param<'world>(world: &'world Ecs) -> Self::Item<'world> {
        query::Query::new(world, ())
    }
}
