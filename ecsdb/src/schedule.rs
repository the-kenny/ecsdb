use std::borrow::Cow;

use crate::{BoxedSystem, Ecs, IntoSystem, LastResult, LastRun, System, SystemResult, system};

use tracing::{debug, debug_span, instrument, warn};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SystemStatus {
    Enabled,
    Disabled,
}

#[derive(Default)]
pub struct Schedule {
    systems: Vec<(BoxedSystem, Box<dyn SchedulingMode>, SystemStatus)>,
}

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
        self.systems.push((
            system.into_boxed_system(),
            Box::new(mode),
            SystemStatus::Enabled,
        ));
        self
    }

    pub fn remove<Marker, S>(&mut self, system: S) -> &mut Self
    where
        S: IntoSystem<Marker>,
        S::System: 'static,
    {
        let system_name = system.into_system().name();
        self.remove_by_name(system_name)
    }

    pub fn remove_by_name(&mut self, system_name: impl AsRef<str>) -> &mut Self {
        let system_name = system_name.as_ref();
        self.systems.retain(|s| s.0.name() != system_name);
        self
    }

    pub fn system_names(&self) -> impl Iterator<Item = Cow<'static, str>> {
        self.systems().map(System::name)
    }

    pub fn systems<'a>(&'a self) -> impl Iterator<Item = &'a BoxedSystem> + 'a {
        self.iter().map(|(s, _m)| s)
    }

    pub fn iter<'a>(
        &'a self,
    ) -> impl Iterator<Item = (&'a BoxedSystem, &'a Box<dyn SchedulingMode>)> + 'a {
        self.systems
            .iter()
            .map(|(system, schedule, _)| (system, schedule))
    }
}

// impl Schedule {
//     pub fn disable_system<S, Marker>(&mut self, system: S) -> &mut self {}
// }

impl Schedule {
    #[instrument(level = "debug", skip_all, ret, err)]
    pub fn tick<'a>(&'a self, ecs: &Ecs) -> Result<Vec<(Cow<'a, str>, TickResult)>, anyhow::Error> {
        let mut results = Vec::with_capacity(self.systems.len());

        for (system, schedule, mode) in self.systems.iter() {
            let _span = debug_span!("system", name = %system.name()).entered();

            let result =
                if *mode == SystemStatus::Enabled && schedule.should_run(ecs, &system.name()) {
                    match ecs.run_dyn_system(system) {
                        Ok(()) => TickResult::Ok,
                        Err(e) => {
                            warn!(error = %e, "System failed");
                            TickResult::Error(e)
                        }
                    }
                } else {
                    debug!("skipping");
                    TickResult::NotScheduled
                };

            debug!(?result);
            results.push((system.name(), result));
        }

        Ok(results)
    }

    /// Run a single system by name, bypassing its scheduling mode.
    ///
    /// Returns [`UnknownSystemError`] if no system with the given name exists
    /// in this schedule.
    pub fn run_system(&self, ecs: &Ecs, name: &str) -> Result<TickResult, UnknownSystemError> {
        let Some((system, _schedule, mode)) = self
            .systems
            .iter()
            .find(|(system, _schedule, _mode)| system.name() == name)
        else {
            return Err(UnknownSystemError(name.into()));
        };

        if *mode == SystemStatus::Disabled {
            warn!(system = %system.name(), "Running disabled system")
        }

        match ecs.run_dyn_system(system) {
            Ok(()) => Ok(TickResult::Ok),
            Err(e) => Ok(TickResult::Error(e)),
        }
    }
}

impl Schedule {
    pub fn enable<S, Marker>(&mut self, system: S) -> &mut Self
    where
        S: IntoSystem<Marker>,
    {
        self.enable_by_name(system.into_system().name())
    }

    pub fn disable<S, Marker>(&mut self, system: S) -> &mut Self
    where
        S: IntoSystem<Marker>,
    {
        self.disable_by_name(system.into_system().name())
    }

    pub fn enable_by_name(&mut self, system_name: impl AsRef<str>) -> &mut Self {
        self.change_mode(system_name, SystemStatus::Enabled);
        self
    }

    pub fn disable_by_name(&mut self, system_name: impl AsRef<str>) -> &mut Self {
        self.change_mode(system_name, SystemStatus::Disabled);
        self
    }

    fn change_mode(&mut self, system_name: impl AsRef<str>, new_mode: SystemStatus) {
        for (_system, _schedule, mode) in self
            .systems
            .iter_mut()
            .filter(|(system, _, _)| system.name() == system_name.as_ref())
        {
            *mode = new_mode;
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Unknown System '{0}'")]
pub struct UnknownSystemError(pub String);

#[derive(Debug)]
pub enum TickResult {
    Ok,
    NotScheduled,
    Error(anyhow::Error),
}

pub trait SchedulingMode: std::fmt::Debug + 'static {
    fn should_run(&self, ecs: &crate::Ecs, system: &str) -> bool;
    fn did_run(&self, _ecs: &crate::Ecs, _system: &str) {}
}

#[derive(Debug)]
pub struct Manually;

impl SchedulingMode for Manually {
    #[instrument(level = "debug", skip_all, fields(self), ret)]
    fn should_run(&self, _ecs: &crate::Ecs, _system: &str) -> bool {
        false
    }
}

#[derive(Debug)]
pub struct Always;

impl SchedulingMode for Always {
    #[instrument(level = "debug", skip_all, fields(self), ret)]
    fn should_run(&self, _ecs: &crate::Ecs, _system: &str) -> bool {
        true
    }
}

#[derive(Debug)]
pub struct Every(pub chrono::Duration);

impl SchedulingMode for Every {
    #[instrument(level = "debug", skip_all, fields(self), ret)]
    fn should_run(&self, ecs: &crate::Ecs, system: &str) -> bool {
        ecs.system_entity(system)
            .and_then(|e| e.component::<system::LastRun>())
            .map(|last_run| {
                debug!(?last_run);
                chrono::Utc::now().signed_duration_since(last_run.0) > self.0
            })
            .unwrap_or(true)
    }
}

#[derive(Debug)]
pub struct Once;

impl SchedulingMode for Once {
    #[instrument(level = "debug", skip_all, fields(self), ret)]
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
    #[instrument(level = "debug", skip_all, fields(self), ret)]
    fn should_run(&self, ecs: &crate::Ecs, system: &str) -> bool {
        let Some(predecessor) = ecs.system_entity(&self.0) else {
            debug!(reason = "Predecessor system not found", "skipping");
            return false;
        };

        let predecessor_last_run = predecessor.component::<LastRun>();
        let predecessor_last_result = predecessor.component::<LastResult>();

        let our_last_run = ecs
            .system_entity(system)
            .and_then(|e| e.component::<LastRun>());

        debug!(
            ?our_last_run,
            ?predecessor_last_run,
            ?predecessor_last_result
        );

        match (predecessor_last_run, predecessor_last_result, our_last_run) {
            // Predecessor never ran: nothing to run after.
            (None, _, _) => false,
            // Predecessor ran but did not succeed: do not run after it.
            (Some(_), None | Some(LastResult(SystemResult::Err(_) | SystemResult::Skipped)), _) => {
                false
            }
            // Predecessor succeeded and we have never run: run.
            (Some(_), Some(LastResult(SystemResult::Ok(_))), None) => true,
            // Predecessor succeeded again after our last run: run again.
            (
                Some(LastRun(predecessor_run)),
                Some(LastResult(SystemResult::Ok(_))),
                Some(LastRun(our_run)),
            ) if predecessor_run > our_run => true,
            // Predecessor's successful run is not newer than ours: already up to date.
            (Some(_), Some(LastResult(SystemResult::Ok(_))), Some(_)) => false,
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
                    sys.modify_component(|Count(c)| *c += 1);
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

    #[test]
    fn tick_results() {
        #[rustfmt::skip]
        fn system_ok(sys: SystemEntity<'_>) { sys.modify_component(|Count( c)| *c += 1); }
        #[rustfmt::skip]
        fn system_error(_sys: SystemEntity<'_>) -> Result<(), anyhow::Error> { Err(anyhow::anyhow!("Expected test error")) }

        let mut schedule = Schedule::new();
        schedule.add(system_ok, Always);
        schedule.add(system_error, Always);
        schedule.add(system_ok, Manually);

        let ecs = Ecs::open_in_memory().unwrap();
        let results = schedule.tick(&ecs).unwrap();

        assert_eq!(results.len(), 3);

        dbg!(&results);

        // First system should run successfully
        assert!(matches!(results[0].1, TickResult::Ok));

        // Second system should return an error
        assert!(matches!(results[1].1, TickResult::Error(_)));

        // Third system should be skipped due to Manually scheduling
        assert!(matches!(results[2].1, TickResult::NotScheduled));
    }

    #[test]
    fn after_does_not_run_when_predecessor_fails() {
        #[rustfmt::skip]
        fn predecessor(_sys: SystemEntity<'_>) -> Result<(), anyhow::Error> { Err(anyhow::anyhow!("Expected test error")) }
        #[rustfmt::skip]
        fn dependent(sys: SystemEntity<'_>) { sys.modify_component(|Count(c)| *c += 1); }

        let mut schedule = Schedule::new();
        schedule.add(predecessor, Always);
        schedule.add(dependent, After::system(predecessor));

        let ecs = Ecs::open_in_memory().unwrap();
        schedule.tick(&ecs).unwrap();
        schedule.tick(&ecs).unwrap();

        // Predecessor keeps failing, so `dependent` must never run.
        assert_eq!(
            ecs.system_entity(&system_name(dependent))
                .and_then(|e| e.component::<Count>()),
            None
        );
    }

    #[test]
    fn after_runs_once_per_successful_predecessor_run() {
        #[rustfmt::skip]
        fn predecessor(_sys: SystemEntity<'_>) {}
        #[rustfmt::skip]
        fn dependent(sys: SystemEntity<'_>) { sys.modify_component(|Count(c)| *c += 1); }

        let mut schedule = Schedule::new();
        schedule.add(predecessor, Always);
        schedule.add(dependent, After::system(predecessor));

        let ecs = Ecs::open_in_memory().unwrap();

        fn dependent_count(ecs: &Ecs) -> Count {
            ecs.system_entity(&system_name(dependent))
                .unwrap()
                .component()
                .unwrap()
        }

        // Predecessor runs every tick; dependent should run once after each.
        schedule.tick(&ecs).unwrap();
        assert_eq!(dependent_count(&ecs), Count(1));

        schedule.tick(&ecs).unwrap();
        assert_eq!(dependent_count(&ecs), Count(2));
    }
}
