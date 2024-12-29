use std::borrow::Cow;

use tracing::debug;

use crate::{query, Ecs};

pub trait System<In, Out = ()>: 'static + Send + Sync {
    fn name(&self) -> Cow<'static, str>;
    fn run(&self, ecs: &Ecs) -> Out;
}

struct ErasedSystem<A, B>(Box<dyn System<A, B>>);

impl<In, Out> System<(), ()> for ErasedSystem<In, Out>
where
    In: 'static,
    Out: 'static,
{
    fn name(&self) -> Cow<'static, str> {
        self.0.name()
    }

    fn run(&self, ecs: &Ecs) -> () {
        self.0.run(ecs);
    }
}

impl<Fun, Filter> System<query::Query<'_, Filter>, ()> for Fun
where
    Fun: Fn(query::Query<Filter>) -> () + Send + Sync + 'static,
    Filter: query::Filter + 'static,
{
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed(std::any::type_name::<Self>())
    }

    fn run(&self, ecs: &Ecs) {
        let query = query::Query::new(ecs, ());
        self(query);
    }
}

impl<Fun, Filter> System<query::Query<'_, Filter>, Result<(), anyhow::Error>> for Fun
where
    Fun: Fn(query::Query<Filter>) -> Result<(), anyhow::Error> + Send + Sync + 'static,
    Filter: query::Filter + 'static,
{
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed(std::any::type_name::<Self>())
    }

    fn run(&self, ecs: &Ecs) -> Result<(), anyhow::Error> {
        let query = query::Query::new(ecs, ());
        self(query)
    }
}

impl Ecs {
    pub fn register<In, Out, S>(&mut self, system: S)
    where
        S: System<In, Out>,
        In: 'static,
        Out: 'static,
    {
        let erased = Box::new(ErasedSystem(Box::new(system)));
        self.systems.push(erased);
    }

    pub fn tick(&self) {
        for system in &self.systems {
            let _span = tracing::info_span!("system", name = system.name().as_ref()).entered();
            let started = std::time::Instant::now();
            debug!("Running");
            system.run(&self);
            debug!(elapsed_ms = started.elapsed().as_millis(), "Finished",);
        }
    }
}
