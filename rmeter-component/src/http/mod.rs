//! HTTP/HTTPS sampling and scoped request defaults.

mod config;
mod plan;
pub(crate) mod sampler;
pub(crate) mod template;

pub use config::{HttpBody, HttpConfigError, HttpRequestConfig, HttpRequestDefaults, Setting};
pub use plan::{HttpNode, compile_http_nodes};
pub use reqwest::Method as HttpMethod;
pub use sampler::HttpSampler;

pub use HttpRequestDefaults as HTTPRequestDefaults;
pub use HttpSampler as HTTPSampler;
