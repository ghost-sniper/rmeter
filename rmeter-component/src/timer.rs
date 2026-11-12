//! Scoped, cancellable pauses between preprocessors and sampler execution.
use rmeter_core::{context::UserContext, sampler::SampleResult};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};

// Preserve integer milliseconds across native JSON and desktop JavaScript.
const MAX_MS: f64 = 9_007_199_254_740_991.0;

/// Milliseconds specified directly or expanded from a variable expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TimerValue {
    Number(f64),
    Expression(String),
}
impl From<u32> for TimerValue {
    fn from(value: u32) -> Self {
        Self::Number(value.into())
    }
}
impl TimerValue {
    fn resolve(&self, variables: &HashMap<String, String>, integer: bool) -> Result<f64, String> {
        let number = match self {
            Self::Number(number) => *number,
            Self::Expression(text) => crate::http::template::expand(text, variables)
                .map_err(|e| e.to_string())?
                .trim()
                .parse::<f64>()
                .map_err(|_| "expected milliseconds".to_owned())?,
        };
        if !number.is_finite()
            || !(0.0..=MAX_MS).contains(&number)
            || (integer && number.fract() != 0.0)
        {
            return Err(format!(
                "milliseconds must be {}between 0 and {MAX_MS}",
                if integer { "an integer " } else { "" }
            ));
        }
        Ok(number)
    }
    fn validate(&self, integer: bool) -> Result<(), String> {
        if let Self::Expression(text) = self {
            crate::http::template::validate(text).map_err(|e| e.to_string())?;
            if text.contains("${") {
                return Ok(());
            }
        }
        self.resolve(&HashMap::new(), integer).map(|_| ())
    }
}

/// A scoped constant or Gaussian delay applied before sampling.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimerConfig {
    ConstantTimer {
        name: String,
        delay_ms: TimerValue,
    },
    GaussianRandomTimer {
        name: String,
        deviation_ms: TimerValue,
        offset_ms: TimerValue,
    },
}
impl TimerConfig {
    /// Return the configured diagnostic label.
    pub fn name(&self) -> &str {
        match self {
            Self::ConstantTimer { name, .. } | Self::GaussianRandomTimer { name, .. } => name,
        }
    }
    /// Validate names, static numbers and templates; dynamic values wait until runtime.
    pub fn validate(&self) -> Result<(), String> {
        if self.name().trim().is_empty() {
            return Err("timer name must not be empty".into());
        }
        let result = match self {
            Self::ConstantTimer { delay_ms, .. } => delay_ms.validate(true),
            Self::GaussianRandomTimer {
                deviation_ms,
                offset_ms,
                ..
            } => deviation_ms
                .validate(false)
                .and_then(|_| offset_ms.validate(true)),
        };
        result.map_err(|e| format!("timer {:?}: {e}", self.name()))
    }
    fn delay(&self, variables: &HashMap<String, String>) -> Result<u64, String> {
        match self {
            Self::ConstantTimer { delay_ms, .. } => Ok(delay_ms.resolve(variables, true)? as u64),
            Self::GaussianRandomTimer {
                deviation_ms,
                offset_ms,
                ..
            } => {
                let deviation = deviation_ms.resolve(variables, false)?;
                let offset = offset_ms.resolve(variables, true)?;
                // Box-Muller: one independent N(0, 1) draw per timer per sample.
                let z = (-2.0 * (1.0 - rand::random::<f64>()).ln()).sqrt()
                    * (std::f64::consts::TAU * rand::random::<f64>()).cos();
                Ok(gaussian_delay(z, deviation, offset))
            }
        }
    }
}
fn gaussian_delay(z: f64, deviation: f64, offset: f64) -> u64 {
    // JMeter applies abs to the total (including offset), then truncates.
    ((z * deviation + offset).abs().min(MAX_MS)) as u64
}
pub(crate) fn validate(timers: &[TimerConfig]) -> Result<(), String> {
    timers.iter().try_for_each(TimerConfig::validate)
}
async fn pause(timers: &[TimerConfig], user: &UserContext) -> Result<(), String> {
    let total = timers.iter().try_fold(0u64, |sum, timer| {
        let delay = timer
            .delay(&user.variables)
            .map_err(|e| format!("timer {:?}: {e}", timer.name()))?;
        sum.checked_add(delay)
            .ok_or_else(|| "total timer delay overflow".to_owned())
    })?;
    // Bound each deadline to avoid platform Instant overflow for large delays.
    // Dropping this future cancels the sleep immediately, without helper tasks.
    let mut remaining = total;
    while remaining != 0 {
        let chunk = remaining.min(86_400_000);
        tokio::time::sleep(Duration::from_millis(chunk)).await;
        remaining -= chunk;
    }
    Ok(())
}
pub(crate) async fn run(
    timers: &[TimerConfig],
    user: &UserContext,
    result: &mut SampleResult,
) -> bool {
    match pause(timers, user).await {
        Ok(()) => true,
        Err(message) => {
            result.success = false;
            result.elapsed = Duration::ZERO;
            result.response_code = Some("TimerError".into());
            result.response_message = Some(message);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gaussian_matches_jmeter_abs_and_truncation() {
        assert_eq!(gaussian_delay(-2.0, 100.0, 50.0), 150);
        assert_eq!(gaussian_delay(1.25, 10.0, 20.0), 32);
        assert_eq!(gaussian_delay(2.0, 0.0, 25.0), 25);
    }
    #[test]
    fn gaussian_draws_vary_and_have_expected_folded_distribution() {
        let config = TimerConfig::GaussianRandomTimer {
            name: "random".into(),
            deviation_ms: 1000.into(),
            offset_ms: 0.into(),
        };
        let values: Vec<_> = (0..20000)
            .map(|_| config.delay(&HashMap::new()).unwrap())
            .collect();
        let mean = values.iter().sum::<u64>() as f64 / values.len() as f64;
        assert!((750.0..850.0).contains(&mean), "half-normal mean: {mean}");
        assert!(values.iter().any(|v| *v > 2000));
        assert!(values.iter().any(|v| *v < 10));
    }
    #[tokio::test(start_paused = true)]
    async fn delays_sum_and_users_wait_independently() {
        let timers = [
            TimerConfig::ConstantTimer {
                name: "a".into(),
                delay_ms: 40.into(),
            },
            TimerConfig::GaussianRandomTimer {
                name: "b".into(),
                deviation_ms: 0.into(),
                offset_ms: 60.into(),
            },
        ];
        let a = UserContext::new("g", 0);
        let b = UserContext::new("g", 1);
        let started = tokio::time::Instant::now();
        let (a, b) = tokio::join!(pause(&timers, &a), pause(&timers, &b));
        a.unwrap();
        b.unwrap();
        assert_eq!(started.elapsed(), Duration::from_millis(100));
    }
    #[tokio::test(start_paused = true)]
    async fn zero_delay_and_invalid_values_do_not_wait() {
        let user = UserContext::new("g", 0);
        let started = tokio::time::Instant::now();
        pause(
            &[TimerConfig::ConstantTimer {
                name: "zero".into(),
                delay_ms: 0.into(),
            }],
            &user,
        )
        .await
        .unwrap();
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert!(
            pause(
                &[TimerConfig::ConstantTimer {
                    name: "bad".into(),
                    delay_ms: TimerValue::Expression("${missing}".into())
                }],
                &user
            )
            .await
            .is_err()
        );
        assert_eq!(started.elapsed(), Duration::ZERO);
    }
}
