use std::{collections::HashMap, time::Duration};

use rmeter_component::{HttpNode, HttpRequestConfig, HttpRequestDefaults, compile_http_nodes};
use rmeter_core::{
    engine::Engine,
    event::RunEvent,
    plan::{ErrorPolicy, LoopCount, TestPlan, ThreadGroup},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:8080/".into());
    let children = compile_http_nodes(
        vec![HttpNode::Request(HttpRequestConfig::with_url(
            "HTTP request",
            url,
        ))],
        &HttpRequestDefaults::default(),
    )?;
    let engine = Engine::new(TestPlan {
        name: "HTTP example".into(),
        variables: HashMap::new(),
        groups: vec![ThreadGroup {
            name: "HTTP users".into(),
            users: 1,
            ramp_up: Duration::ZERO,
            iterations: LoopCount::Finite(1),
            max_duration: None,
            on_error: ErrorPolicy::Continue,
            children,
        }],
    });
    let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
    let (result, ()) = tokio::join!(engine.run(sender), async {
        while let Some(event) = receiver.recv().await {
            match event {
                RunEvent::Started { .. } => println!("Started"),
                RunEvent::SampleCompleted(sample) => println!(
                    "{}: success={}, status={:?}, elapsed={:?}, received={} bytes, message={:?}",
                    sample.result.label,
                    sample.result.success,
                    sample.result.response_code,
                    sample.result.elapsed,
                    sample.result.bytes_received,
                    sample.result.response_message,
                ),
                RunEvent::Finished { .. } => println!("Finished"),
            }
        }
    });
    result?;
    Ok(())
}
