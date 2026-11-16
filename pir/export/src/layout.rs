//! Publisher layouts keep 4,096 rows and widen the private tier once needed.

use anyhow::{ensure, Context, Result};
use pir_types::{PirLayout, COMPILED_PIR_LAYOUT, PIR_DEPTH, TIER0_LAYERS};

/// Largest publisher layout qualified against the existing HTTP query limit.
///
/// Depth 20 uses 12 public and 8 private layers. Do not automatically grow
/// beyond this layout without checking server and wallet resource limits.
pub const MAX_EXPORT_DEPTH: usize = 20;

pub(crate) fn layout_for_depth(depth: usize) -> Result<PirLayout> {
    ensure!(
        (PIR_DEPTH..=MAX_EXPORT_DEPTH).contains(&depth),
        "unsupported publisher tree depth {depth}; expected {PIR_DEPTH}..={MAX_EXPORT_DEPTH}"
    );
    let layout = PirLayout {
        pir_depth: depth,
        tier1_layers: depth - TIER0_LAYERS,
        ..COMPILED_PIR_LAYOUT
    };
    layout
        .validate_supported()
        .map_err(anyhow::Error::msg)
        .context("invalid publisher layout")?;
    Ok(layout)
}

pub(crate) fn layout_for_ranges(count: usize) -> Result<PirLayout> {
    ensure!(
        count <= 1 << MAX_EXPORT_DEPTH,
        "too many ranges ({count}) for publisher depth {MAX_EXPORT_DEPTH} (max {}): revise the PIR layout before publishing",
        1 << MAX_EXPORT_DEPTH
    );
    let mut depth = PIR_DEPTH;
    while count > 1 << depth {
        depth += 1;
    }
    layout_for_depth(depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_existing_layout_until_full_then_widen_private_rows() {
        for count in [0, 487_990, 1 << PIR_DEPTH] {
            assert_eq!(layout_for_ranges(count).unwrap(), COMPILED_PIR_LAYOUT);
        }
        for count in [(1 << PIR_DEPTH) + 1, 619_172, 1 << 20] {
            let layout = layout_for_ranges(count).unwrap();
            assert_eq!(layout.pir_depth, 20);
            assert_eq!(layout.tier0_layers, 12);
            assert_eq!(layout.tier1_layers, 8);
            assert_eq!(layout.tier1_rows().unwrap(), 4096);
            assert_eq!(layout.tier1_row_bytes().unwrap(), 24576);
        }
    }

    #[test]
    fn reject_growth_beyond_the_12_plus_8_layout() {
        for depth in PIR_DEPTH..=MAX_EXPORT_DEPTH {
            assert_eq!(layout_for_ranges(1 << depth).unwrap().pir_depth, depth);
            if depth < MAX_EXPORT_DEPTH {
                assert_eq!(
                    layout_for_ranges((1 << depth) + 1).unwrap().pir_depth,
                    depth + 1
                );
            }
        }
        assert!(layout_for_ranges((1 << MAX_EXPORT_DEPTH) + 1).is_err());
        assert!(layout_for_ranges(usize::MAX).is_err());
        // The former maximum generated a request eight bytes over the HTTP limit.
        assert!(layout_for_ranges((1 << 22) + 1).is_err());
        assert!(layout_for_depth(23).is_err());
    }
}
