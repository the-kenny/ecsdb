use tracing::{debug, error, info_span};

use crate::{query, Ecs};

use core::marker::PhantomData;
use std::borrow::Cow;

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
        SystemParamFunction::run(&self.system, F::Param::get_param(app)).into_result()
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
        eprintln!("calling a function with no params");
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

            fn get_param<'world>(world: &'world Ecs) -> Self::Item<'world> {
                ($($param::get_param(world),)*)
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

// impl<F, Out, P1> SystemParamFunction<(P1,)> for F
// where
//     F: Send + Sync + 'static,
//     for<'a> &'a F: Fn(P1) -> Out + Fn(<P1 as SystemParam>::Item<'_>) -> Out,
//     P1: SystemParam,
//     Out: SystemOutput,
// {
//     type Param = (P1,);
//     fn run(&self, p1: <Self::Param as SystemParam>::Item<'_>) {
//         // #[allow(clippy::too_many_arguments)]
//         // fn call_inner<Out, P1>(f: impl Fn(P1) -> Out, p1: P1) -> Out {
//         //     f(p1)
//         // }
//         // let (p1,) = p1;
//         // call_inner(self, p1);

//         (&self)(p1.0);
//     }
// }

// impl<F, Out, P1, P2> SystemParamFunction<(P1, P2)> for F
// where
//     F: Send + Sync + 'static,
//     for<'a> &'a F:
//         Fn(P1, P2) -> Out + Fn(<P1 as SystemParam>::Item<'_>, <P2 as SystemParam>::Item<'_>) -> Out,
//     P1: SystemParam,
//     P2: SystemParam,
//     Out: SystemOutput,
// {
//     type Param = (P1, P2);
//     fn run(&self, p: <Self::Param as SystemParam>::Item<'_>) {
//         (&self)(p.0, p.1);
//     }
// }

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

// impl<T1: SystemParam> SystemParam for (T1,) {
//     type Item<'world> = (T1::Item<'world>,);

//     fn get_param<'world>(world: &'world Ecs) -> Self::Item<'world> {
//         (T1::get_param(world),)
//     }
// }

// impl<T1: SystemParam, T2: SystemParam> SystemParam for (T1, T2) {
//     type Item<'world> = (T1::Item<'world>, T2::Item<'world>);

//     fn get_param<'world>(world: &'world Ecs) -> Self::Item<'world> {
//         (T1::get_param(world), T2::get_param(world))
//     }
// }

impl Ecs {
    pub fn tick(&self) {
        for system in &self.systems {
            let _span = tracing::info_span!("system", name = system.name().as_ref()).entered();
            let started = std::time::Instant::now();
            debug!("Running");
            if let Err(e) = system.run(&self) {
                error!(?e);
            }

            debug!(elapsed_ms = started.elapsed().as_millis(), "Finished",);
        }
    }

    pub fn register<F: IntoSystem<Params>, Params: SystemParam>(&mut self, system: F) {
        self.systems.push(Box::new(system.into_system()));
    }

    pub fn system<'a>(&'a self, name: &str) -> Option<&'a dyn System> {
        self.systems
            .iter()
            .find(|s| s.name() == name)
            .map(|s| s.as_ref())
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

#[cfg(test)]
mod tests {
    use crate::{query, Ecs};

    #[test]
    fn no_param() {
        let mut ecs = Ecs::open_in_memory().unwrap();
        ecs.register(|| ());
    }

    #[test]
    fn ecs_param() {
        let mut ecs = Ecs::open_in_memory().unwrap();
        ecs.register(|_ecs: &Ecs| ());
    }

    #[test]
    fn query_param() {
        let mut ecs = Ecs::open_in_memory().unwrap();
        ecs.register(|_q: query::Query<()>| ());
    }

    #[test]
    fn multiple_params() {
        let mut ecs = Ecs::open_in_memory().unwrap();
        ecs.register(|_ecs: &Ecs, _q: query::Query<()>| ());
        ecs.register(|_: &Ecs, _: &Ecs| ());
        ecs.register(|_: &Ecs, _: &Ecs, _: &Ecs| ());
        ecs.register(|_: &Ecs, _: &Ecs, _: &Ecs, _: &Ecs| ());
        ecs.register(|_: &Ecs, _: &Ecs, _: &Ecs, _: &Ecs, _: &Ecs| ());
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
        let mut db = Ecs::open_in_memory().unwrap();
        fn system(query: query::Query<(A, B)>) {
            for entity in query.try_iter().unwrap() {
                entity.attach(Seen);
            }
        }

        db.register(system);

        let a_and_b = db.new_entity().attach(A).attach(B);
        let a = db.new_entity().attach(A);

        db.tick();
        assert!(a_and_b.component::<Seen>().is_some());
        assert!(a.component::<Seen>().is_none());
    }

    #[test]
    fn run_ecs() {
        let mut db = Ecs::open_in_memory().unwrap();
        fn system(ecs: &Ecs) {
            ecs.new_entity().attach(Seen);
        }

        db.register(system);
        db.tick();

        assert!(db.find(Seen).next().is_some());
    }
}
