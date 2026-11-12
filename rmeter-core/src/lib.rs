#![doc = include_str!("../docs/en.md")]

#[cfg(doc)]
pub mod translations;

pub mod assertion;
pub mod context;
pub mod controller;
pub mod engine;
pub mod event;
pub mod plan;
pub mod sampler;
/// Add two unsigned integers; panics on overflow when overflow checks are enabled.
pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
