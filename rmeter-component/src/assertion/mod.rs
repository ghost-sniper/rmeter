//! Response assertions, independent of transport components.

mod json;

pub(crate) use json::evaluate_json_assertions;
pub use json::{
    JsonAssertion, JsonAssertionConfig, JsonAssertionError, JsonMatcher, MatchSelection,
};
