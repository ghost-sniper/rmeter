//! Shared processors for all samplers; no additional sample events.
use crate::{JavaScriptConfig, JavaScriptConfigError, JavaScriptSampler};
use rmeter_core::{context::UserContext, sampler::SampleResult};
use std::sync::Arc;

pub(crate) fn compile(
    configs: &[JavaScriptConfig],
) -> Result<Vec<JavaScriptSampler>, JavaScriptConfigError> {
    configs
        .iter()
        .cloned()
        .map(|config| {
            if !config.pre_processors.is_empty()
                || !config.post_processors.is_empty()
                || !config.timers.is_empty()
            {
                return Err(JavaScriptConfigError(
                    "processors cannot contain processors or timers".into(),
                ));
            }
            JavaScriptSampler::new(config)
        })
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
    // Snapshot the parent result before assertions. Processor writes only affect
    // user variables, and never change the result seen by subsequent processors.
    let previous = Arc::new(serde_json::json!({
        "label": result.label, "success": result.success,
        "responseCode": result.response_code, "responseMessage": result.response_message,
        "elapsedMs": result.elapsed.as_secs_f64() * 1000.0,
        "body": body, "bodyUnavailableReason": reason,
    }));
    for processor in processors {
        if let Err((code, message)) = processor
            .run(user, Some(previous.clone()), &result.label)
            .await
        {
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

/// A failed preprocessor aborts the parent request and the remaining chain.
pub(crate) async fn run_pre(
    processors: &[JavaScriptSampler],
    user: &mut UserContext,
    result: &mut SampleResult,
) -> bool {
    for processor in processors {
        if let Err((code, message)) = processor.run(user, None, &result.label).await {
            result.success = false;
            result.response_code = Some(code.into());
            result.response_message = Some(format!(
                "pre processor {:?} failed ({code}): {message}",
                processor.config().name
            ));
            return false;
        }
    }
    true
}

/// An HTTP postprocessor: JavaScript or a native response extractor.
#[derive(Debug, Clone)]
pub enum HttpPostProcessorConfig {
    JavaScript(JavaScriptConfig),
    Extractor(crate::extractor::ExtractorConfig),
}
impl From<JavaScriptConfig> for HttpPostProcessorConfig {
    fn from(value: JavaScriptConfig) -> Self {
        Self::JavaScript(value)
    }
}
impl From<crate::extractor::ExtractorConfig> for HttpPostProcessorConfig {
    fn from(value: crate::extractor::ExtractorConfig) -> Self {
        Self::Extractor(value)
    }
}
#[derive(Debug, Clone)]
pub(crate) enum HttpPostProcessor {
    JavaScript(JavaScriptSampler),
    Extractor(crate::extractor::Extractor),
}
pub(crate) fn compile_http(
    configs: &[HttpPostProcessorConfig],
) -> Result<Vec<HttpPostProcessor>, JavaScriptConfigError> {
    configs
        .iter()
        .map(|config| match config {
            HttpPostProcessorConfig::JavaScript(config) => {
                let mut scripts = compile(std::slice::from_ref(config))?;
                Ok(HttpPostProcessor::JavaScript(scripts.remove(0)))
            }
            HttpPostProcessorConfig::Extractor(config) => Ok(HttpPostProcessor::Extractor(
                crate::extractor::Extractor::new(config.clone())?,
            )),
        })
        .collect()
}
pub(crate) async fn run_http(
    processors: &[HttpPostProcessor],
    user: &mut UserContext,
    result: &mut SampleResult,
    body: Option<&[u8]>,
    reason: Option<&str>,
) {
    let (body, reason) = match body {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => (Some(text), None),
            Err(_) => (None, Some("response body is not valid UTF-8")),
        },
        None => (
            None,
            Some(reason.unwrap_or("complete response body is unavailable")),
        ),
    };
    let previous = Arc::new(serde_json::json!({
        "label":result.label,"success":result.success,"responseCode":result.response_code,"responseMessage":result.response_message,
        "elapsedMs":result.elapsed.as_secs_f64()*1000.0,"body":body,"bodyUnavailableReason":reason,
    }));
    for processor in processors {
        let (name, outcome) = match processor {
            HttpPostProcessor::JavaScript(script) => (
                &script.config().name,
                script
                    .run(user, Some(previous.clone()), &result.label)
                    .await,
            ),
            HttpPostProcessor::Extractor(extractor) => (
                &extractor.config().settings.name,
                extractor.run(user, body, reason).await,
            ),
        };
        if let Err((code, message)) = outcome {
            result.success = false;
            let diagnostic = format!("post processor {name:?} failed ({code}): {message}");
            result.response_message = Some(match result.response_message.take() {
                Some(original) => format!("{original}; {diagnostic}"),
                None => diagnostic,
            });
            break;
        }
    }
}
