//! snakewood-core: the pure, deterministic world model and storage layer.

pub mod direction;

pub use direction::Direction;

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds() {
        assert_eq!(2 + 2, 4);
    }
}
