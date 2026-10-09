#![doc = include_str!("../README.md")]

pub mod assertion;
pub mod context;
pub mod controller;
pub mod engine;
pub mod event;
pub mod plan;
pub mod sampler;
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
