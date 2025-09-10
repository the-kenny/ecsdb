use crate::{system, BoxedSystem, Ecs, IntoSystem, LastRun, System};

use tracing::{debug, info, instrument};

#[derive(Default)]
pub struct Schedule(Vec<(BoxedSystem, Box<dyn SchedulingMode>)>);

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add<Marker, S, M>(&mut self, system: S, mode: M) -> &mut Self
    where
        S: IntoSystem<Marker>,
        S::System: 'static,
        M: SchedulingMode,
    {
        self.0.push((system.into_boxed_system(), Box::new(mode)));
        self
    }

    #[instrument(level = "debug", skip_all)]
    pub fn tick(&self, ecs: &Ecs) -> Result<(), anyhow::Error> {
        for (system, schedule) in self.0.iter() {
            if schedule.should_run(&ecs, &system.name()) {
                info!(system = %system.name(), "running");
                ecs.run_dyn_system(system)?;
            } else {
                debug!(system = %system.name(), "skipping")
            }
        }

        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = &(BoxedSystem, Box<dyn SchedulingMode>)> {
        self.0.iter()
    }
}

pub trait SchedulingMode: std::fmt::Debug + 'static {
    fn should_run(&self, ecs: &crate::Ecs, system: &str) -> bool;
    fn did_run(&self, _ecs: &crate::Ecs, _system: &str) {}
}

#[derive(Debug)]
pub struct Manually;

impl SchedulingMode for Manually {
    fn should_run(&self, _ecs: &crate::Ecs, _system: &str) -> bool {
        false
    }
}

#[derive(Debug)]
pub struct Always;

impl SchedulingMode for Always {
    fn should_run(&self, _ecs: &crate::Ecs, _system: &str) -> bool {
        true
    }
}

#[derive(Debug)]
pub struct Every(pub chrono::Duration);

impl SchedulingMode for Every {
    fn should_run(&self, ecs: &crate::Ecs, system: &str) -> bool {
        ecs.system_entity(system)
            .and_then(|e| e.component::<system::LastRun>())
            .map(|last_run| chrono::Utc::now().signed_duration_since(&last_run.0) > self.0)
            .unwrap_or(true)
    }
}

#[derive(Debug)]
pub struct Once;

impl SchedulingMode for Once {
    fn should_run(&self, ecs: &crate::Ecs, system: &str) -> bool {
        let entity = ecs.get_or_create_system_entity(system);
        entity.component::<system::LastRun>().is_none()
    }
}

#[derive(Debug)]
pub struct After(String);

impl After {
    pub fn system<Marker, S>(system: S) -> Self
    where
        S: IntoSystem<Marker>,
    {
        Self(system.into_system().name().into())
    }
}

impl SchedulingMode for After {
    fn should_run(&self, ecs: &crate::Ecs, system: &str) -> bool {
        let predecessor_last_run = ecs.system_entity(&self.0).and_then(|e| e.component());

        let our_last_run = ecs
            .system_entity(system)
            .and_then(|e| e.component::<LastRun>());

        match (predecessor_last_run, our_last_run) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(LastRun(before)), Some(LastRun(after))) if before > after => true,
            (Some(_), Some(_)) => false,
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{self as ecsdb, SystemEntity};
    use ecsdb_derive::Component;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::system_name;

    #[derive(Serialize, Deserialize, Component, Default, PartialEq, Debug)]
    struct Count(pub usize);

    #[test]
    fn schedules() {
        macro_rules! defsys {
            ($sys:ident) => {
                fn $sys(sys: SystemEntity<'_>) {
                    sys.modify_component(|Count(ref mut c)| *c += 1);
                }
            };
        }

        defsys!(sys_a);
        defsys!(sys_b);
        defsys!(sys_c);

        let mut schedule = Schedule::new();
        schedule.add(sys_a, Once);
        schedule.add(sys_b, After::system(sys_a));
        schedule.add(sys_c, Always);

        let ecs = Ecs::open_in_memory().unwrap();
        schedule.tick(&ecs).unwrap();
        schedule.tick(&ecs).unwrap();

        fn sys_count<Marker>(ecs: &Ecs, sys: impl IntoSystem<Marker>) -> Count {
            ecs.system_entity(&system_name(sys))
                .unwrap()
                .component()
                .unwrap()
        }

        // sys_a should have a count of 1
        assert_eq!(sys_count(&ecs, sys_a), Count(1));

        // sys_b should also have a count of 1
        assert_eq!(sys_count(&ecs, sys_b), Count(1));

        // sys_c should have a count of 2
        assert_eq!(sys_count(&ecs, sys_c), Count(2));
    }
}
