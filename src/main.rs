use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

use clap::Parser;
use rmeter::{
    Engine, load_script,
    report::{ReportEvent, RunSummary},
};
use serde::Serialize;
use tokio::sync::mpsc;

#[derive(Parser)]
#[command(
    version,
    about = "Run a rmeter JSON test script; write JSON Lines events to stdout"
)]
struct Args {
    /// UTF-8 JSON script file
    script: PathBuf,
    /// Validate and compile the script without sending requests
    #[arg(long)]
    check: bool,
    /// Override a user variable; repeat for multiple variables
    #[arg(long = "var", value_name = "KEY=VALUE", value_parser = parse_variable)]
    variables: Vec<(String, String)>,
}
fn parse_variable(text: &str) -> Result<(String, String), String> {
    let (key, value) = text.split_once('=').ok_or("expected KEY=VALUE")?;
    if key.trim().is_empty() {
        return Err("variable key must not be empty".into());
    }
    Ok((key.into(), value.into()))
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    match execute(args).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("rmeter: {error}");
            ExitCode::from(2)
        }
    }
}

async fn execute(args: Args) -> Result<u8, Box<dyn std::error::Error>> {
    let mut plan = load_script(&args.script).await?;
    plan.variables.extend(args.variables);
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    if args.check {
        write_json(
            &mut output,
            &serde_json::json!({ "type": "validated", "name": plan.name }),
        )?;
        return Ok(0);
    }
    let engine = Engine::new(plan);
    let (sender, mut receiver) = mpsc::channel(256);
    let run = async {
        tokio::select! {
            result = engine.run(sender) => result.map(|()| false).map_err(|error| error.to_string()),
            signal = tokio::signal::ctrl_c() => signal.map(|()| true).map_err(|error| error.to_string()),
        }
    };
    let writer = &mut output;
    let consume = async move {
        let mut summary = RunSummary::default();
        while let Some(event) = receiver.recv().await {
            summary.observe(&event);
            write_json(writer, &ReportEvent::from(event))?;
        }
        Ok::<_, io::Error>(summary)
    };
    let (run_result, summary) = tokio::join!(run, consume);
    let summary = summary?;
    let cancelled = run_result.as_ref().is_ok_and(|cancelled| *cancelled);
    write_json(
        &mut output,
        &serde_json::json!({
            "type": "summary", "samples": summary.samples, "passed": summary.passed,
            "failed": summary.failed, "finished": summary.finished, "cancelled": cancelled,
            "error": run_result.as_ref().err(),
        }),
    )?;
    match run_result {
        Err(error) => Err(error.into()),
        Ok(true) => Ok(130),
        Ok(false) => Ok(u8::from(summary.failed > 0)),
    }
}

fn write_json(output: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    writeln!(output)?;
    output.flush()
}
