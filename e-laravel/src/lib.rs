//! Laravel project intelligence for `e`, with no UI: what a project's routes,
//! views, config, translations, models, Livewire and Inertia components are,
//! and the completions, lints, navigation and code generation built on them.
//!
//! Kept apart from the editor so it builds and tests without the GUI stack,
//! and runs against real projects in the corpus job.

pub mod artisan;
pub mod authz;
pub mod blade_vars;
pub mod codegen;
pub mod contract;
#[cfg(test)]
mod corpus;
pub mod eloquent;
pub mod eloquent_helper;
pub mod env_guard;
pub mod events;
pub mod failed_jobs;
pub mod heal;
pub mod ide_json;
pub mod inertia;
pub mod intentions;
pub mod keys;
pub mod laravel;
pub mod laravel_lint;
pub mod laravel_log;
pub mod livewire;
pub mod migration_check;
pub mod move_class;
pub mod phptools;
pub mod policies;
pub mod project_map;
pub mod querycomplete;
pub mod relatedfiles;
pub mod relations;
pub mod request_test;
pub mod test_impact;
pub mod validation;
