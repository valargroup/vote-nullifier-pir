//! Shared test utilities for the imt-tree crate.

use pasta_curves::Fp;

/// Construct an `Fp` from a `u64` literal.
pub fn fp(v: u64) -> Fp {
    Fp::from(v)
}
