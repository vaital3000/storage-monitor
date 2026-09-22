//! Every cleanup module this build ships: one list, which the CLI and the desktop app both
//! read, so that the two can never disagree about what a build can clean.
//!
//! Adding a module is a crate under `crates/modules/<id>` and one line in [`registry`]
//! (`docs/modules/README.md`).

use storage_monitor_core::module::Module;

/// The modules of this build, in the order the Cleanup screen lists them.
///
/// The demo module is a debug build's only (phase 2b design, section 9): a release build of
/// 2b has none, and its Cleanup section says "soon" until the first real module arrives.
pub fn registry() -> Vec<Box<dyn Module>> {
    let mut modules: Vec<Box<dyn Module>> = Vec::new();
    if cfg!(debug_assertions) {
        modules.push(Box::new(storage_monitor_module_demo::Demo));
    }
    modules
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_demo_is_registered_in_debug_builds_only() {
        let ids: Vec<&str> = registry()
            .iter()
            .map(|module| module.descriptor().id)
            .collect();
        if cfg!(debug_assertions) {
            assert!(ids.contains(&storage_monitor_module_demo::ID), "{ids:?}");
        } else {
            assert!(!ids.contains(&storage_monitor_module_demo::ID), "{ids:?}");
        }
    }

    #[test]
    fn module_ids_are_unique() {
        let modules = registry();
        let ids: HashSet<&str> = modules
            .iter()
            .map(|module| module.descriptor().id)
            .collect();
        assert_eq!(
            ids.len(),
            modules.len(),
            "an item id is `<module>:<native id>`"
        );
    }

    #[test]
    fn every_module_names_itself_and_its_tools() {
        for module in registry() {
            let descriptor = module.descriptor();
            assert!(!descriptor.id.is_empty() && !descriptor.id.contains(':'));
            assert!(!descriptor.name.is_empty(), "{}", descriptor.id);
            assert!(!descriptor.description.is_empty(), "{}", descriptor.id);
            assert!(
                descriptor.tools.iter().all(|tool| !tool.contains('/')),
                "{}: a tool is a name the port locates, never a path",
                descriptor.id
            );
        }
    }
}
