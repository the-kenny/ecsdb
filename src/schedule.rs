use crate::{system, IntoSystem, LastRun, System};

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
