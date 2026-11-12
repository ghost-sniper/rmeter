#![doc = include_str!("../docs/en.md")]

#[cfg(doc)]
pub mod translations;

pub mod report;
pub mod script;

pub use rmeter_component as component;
pub use rmeter_component::HttpSampler;
pub use rmeter_component::{
    ComponentSampler, JdbcConnectionConfig, JdbcSampler, JdbcSamplerConfig,
};
pub use rmeter_core as core;
pub use rmeter_core::{engine::Engine, event::RunEvent, plan::TestPlan};
pub use script::{ScriptError, load_script, parse_script};

/// Parse and run JSON with the caller's async runtime. Consume events concurrently.
/// Dropping this future cancels the engine and its virtual users.
///
/// # Errors
/// Returns [`ScriptError`] for invalid input, component compilation failures or
/// engine failures. An unsuccessful sample is delivered through [`RunEvent`]
/// and does not by itself make this function return an error.
pub async fn run_script(
    source: &str,
    events: tokio::sync::mpsc::Sender<RunEvent>,
) -> Result<(), ScriptError> {
    Engine::new(parse_script(source)?).run(events).await?;
    Ok(())
}

/// Load a UTF-8 JSON file and execute it using the same pipeline as the CLI.
///
/// Like [`run_script`], this requires an existing Tokio runtime and a concurrently
/// drained event channel. Dropping the future cancels an in-progress run.
///
/// # Errors
/// Returns [`ScriptError`] for file I/O, parsing, configuration or engine errors.
pub async fn run_script_file(
    path: impl AsRef<std::path::Path>,
    events: tokio::sync::mpsc::Sender<RunEvent>,
) -> Result<(), ScriptError> {
    Engine::new(load_script(path).await?).run(events).await?;
    Ok(())
}
