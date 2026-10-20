//! Shared post-processing for HTTP and JDBC; no additional sample events.
use crate::{JavaScriptConfig, JavaScriptConfigError, JavaScriptSampler};
use rmeter_core::{context::UserContext, sampler::SampleResult};
use std::sync::Arc;

pub(crate) fn compile(
    configs: &[JavaScriptConfig],
) -> Result<Vec<JavaScriptSampler>, JavaScriptConfigError> {
    configs
        .iter()
        .cloned()
        .map(JavaScriptSampler::new)
        .collect()
}

pub(crate) async fn run(
    processors: &[JavaScriptSampler],
    user: &mut UserContext,
    result: &mut SampleResult,
    body: Option<&[u8]>,
    unavailable_reason: Option<&str>,
) {
    if processors.is_empty() {
        return;
    }
    let (body, reason) = match body {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(body) => (Some(body), None),
            Err(_) => (None, Some("response body is not valid UTF-8")),
        },
        None => (
            None,
            Some(unavailable_reason.unwrap_or("complete response body is unavailable")),
        ),
    };
    // Snapshot the parent result after assertions. Processor writes only affect
    // user variables, and never change the result seen by subsequent processors.
    let previous = Arc::new(serde_json::json!({
        "label": result.label, "success": result.success,
        "responseCode": result.response_code, "responseMessage": result.response_message,
        "elapsedMs": result.elapsed.as_secs_f64() * 1000.0,
        "body": body, "bodyUnavailableReason": reason,
    }));
    for processor in processors {
        if let Err((code, message)) = processor.run(user, Some(previous.clone())).await {
            result.success = false;
            let diagnostic = format!(
                "post processor {:?} failed ({code}): {message}",
                processor.config().name
            );
            result.response_message = Some(match result.response_message.take() {
                Some(original) => format!("{original}; {diagnostic}"),
                None => diagnostic,
            });
            break;
        }
    }
}
