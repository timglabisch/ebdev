mod module_loader;
mod ops;
mod runtime;
mod task_runner;

pub use ::ebdev_task_runner::TaskRunnerHandle;
pub use runtime::{load_ts_config, Error};
pub use task_runner::{list_tasks, run_task, complete_arg, list_flags, complete_flag_value, TaskInfo, ArgInfo, FlagInfo, FlagConfigField};

pub const EBDEV_TYPES: &str = include_str!("../types/ebdev.d.ts");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ebdev_types_is_valid() {
        assert!(!EBDEV_TYPES.is_empty());
        assert!(EBDEV_TYPES.contains("declare module"));
        assert!(EBDEV_TYPES.contains("defineConfig"));
    }
}
