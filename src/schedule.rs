use crate::{system, Entity};

pub trait SchedulingMode: std::fmt::Debug {
    fn should_run(&self, ecs: &crate::Ecs, system: Entity) -> bool;
    fn did_run(&self, _ecs: &crate::Ecs, _system: Entity) {}
}

#[derive(Debug)]
pub struct Manually;

impl SchedulingMode for Manually {
    fn should_run(&self, _ecs: &crate::Ecs, _system: Entity) -> bool {
        false
    }
}

#[derive(Debug)]
pub struct Always;

impl SchedulingMode for Always {
    fn should_run(&self, _ecs: &crate::Ecs, _system: Entity) -> bool {
        true
    }
}

#[derive(Debug)]
pub struct Every(pub chrono::Duration);

impl SchedulingMode for Every {
    fn should_run(&self, _ecs: &crate::Ecs, system: Entity) -> bool {
        system
            .component::<system::LastRun>()
            .map(|last_run| chrono::Utc::now().signed_duration_since(&last_run.0) > self.0)
            .unwrap_or(true)
    }
}

#[derive(Debug)]
pub struct Oneshot;

impl SchedulingMode for Oneshot {
    fn should_run(&self, _ecs: &crate::Ecs, system: Entity) -> bool {
        system.component::<system::LastRun>().is_none()
    }
}
