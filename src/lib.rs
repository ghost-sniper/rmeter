#![doc = include_str!("../README.md")]

pub mod report;
pub mod script;

pub use rmeter_component as component;
pub use rmeter_component::HttpSampler;
pub use rmeter_core as core;
pub use rmeter_core::{engine::Engine, event::RunEvent, plan::TestPlan};
pub use script::{ScriptError, load_script, parse_script};

/// Parse and run JSON with the caller's async runtime. Consume events concurrently.
/// Dropping this future cancels the engine and its virtual users.
pub async fn run_script(
    source: &str,
    events: tokio::sync::mpsc::Sender<RunEvent>,
) -> Result<(), ScriptError> {
    Engine::new(parse_script(source)?).run(events).await?;
    Ok(())
}

/// Load a UTF-8 JSON file and execute it using the same pipeline as the CLI.
pub async fn run_script_file(
    path: impl AsRef<std::path::Path>,
    events: tokio::sync::mpsc::Sender<RunEvent>,
) -> Result<(), ScriptError> {
    Engine::new(load_script(path).await?).run(events).await?;
    Ok(())
}
