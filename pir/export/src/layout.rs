//! Publisher layouts keep private rows fixed and grow the public index.

use anyhow::{ensure, Context, Result};
use pir_types::{PirLayout, COMPILED_PIR_LAYOUT, MAX_TIER0_LAYERS, PIR_DEPTH, TIER1_LAYERS};

/// Largest tree the publisher can export while retaining the existing private row size.
pub const MAX_EXPORT_DEPTH: usize = MAX_TIER0_LAYERS + TIER1_LAYERS;

pub(crate) fn layout_for_depth(depth: usize) -> Result<PirLayout> {
    ensure!(
        (PIR_DEPTH..=MAX_EXPORT_DEPTH).contains(&depth),
        "unsupported publisher tree depth {depth}; expected {PIR_DEPTH}..={MAX_EXPORT_DEPTH}"
    );
    let layout = PirLayout {
        pir_depth: depth,
        tier0_layers: depth - TIER1_LAYERS,
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
    fn keep_existing_layout_until_full_then_grow_public_index() {
        for count in [0, 487_990, 1 << PIR_DEPTH] {
            assert_eq!(layout_for_ranges(count).unwrap(), COMPILED_PIR_LAYOUT);
        }
        for count in [(1 << PIR_DEPTH) + 1, 619_172, 1 << 20] {
            let layout = layout_for_ranges(count).unwrap();
            assert_eq!(layout.pir_depth, 20);
            assert_eq!(layout.tier0_layers, 13);
            assert_eq!(layout.tier1_layers, 7);
            assert_eq!(layout.tier1_rows().unwrap(), 8192);
            assert_eq!(layout.tier1_row_bytes().unwrap(), 12288);
        }
    }

    #[test]
    fn respect_client_resource_limits_at_every_boundary() {
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
    }
}
