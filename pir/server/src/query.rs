use anyhow::{Context, Result};
use spiral_rs::params::Params;
use ypir::params::DbRowsCols;

use crate::U64_BYTES;

/// Validate both wire sections before allocating buffers or entering YPIR,
/// which assumes exact query and packing parameter dimensions.
pub(super) fn parse_query<'a>(
    query_bytes: &'a [u8],
    params: &Params,
) -> Result<(&'a [u8], &'a [u8])> {
    anyhow::ensure!(
        query_bytes.len() >= U64_BYTES,
        "query too short: {} bytes",
        query_bytes.len()
    );
    let pqr_byte_len = usize::try_from(u64::from_le_bytes(
        query_bytes[..U64_BYTES].try_into().unwrap(),
    ))
    .context("pqr_byte_len does not fit usize")?;
    let expected_pqr_bytes = params
        .db_rows_padded_simplepir()
        .checked_mul(U64_BYTES)
        .context("packed query size overflow")?;
    anyhow::ensure!(
        pqr_byte_len == expected_pqr_bytes,
        "packed query size mismatch: got {pqr_byte_len} bytes, expected {expected_pqr_bytes}"
    );

    let payload = &query_bytes[U64_BYTES..];
    anyhow::ensure!(
        pqr_byte_len <= payload.len(),
        "pqr_byte_len {} exceeds payload ({})",
        pqr_byte_len,
        payload.len()
    );
    let (pqr, pub_params) = payload.split_at(pqr_byte_len);
    // The client packs one 1 x t_exp_left matrix per expansion level.
    let expected_pub_params_bytes = params
        .poly_len_log2
        .checked_mul(params.t_exp_left)
        .and_then(|n| n.checked_mul(params.poly_len))
        .and_then(|n| n.checked_mul(U64_BYTES))
        .context("public parameter size overflow")?;
    anyhow::ensure!(
        pub_params.len() == expected_pub_params_bytes,
        "pub_params size mismatch: got {} bytes, expected {}",
        pub_params.len(),
        expected_pub_params_bytes
    );
    Ok((pqr, pub_params))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pir_types::serialize_ypir_query;
    use ypir::{
        client::YPIRClient,
        params::{params_for_scenario_simplepir_with_config, YPIRSPConfig},
    };

    #[test]
    fn accepts_client_queries_with_both_degrees_and_row_padding() {
        for (poly_len, parameter_words) in [(2048, 67_584), (4096, 196_608)] {
            for (rows, item_bits, query_words) in [
                (2048, 196_608, poly_len), // Supported 11+8 layout.
                (4096, 98_304, 4096),
                (4097, 98_304, 8192),
            ] {
                let client = YPIRClient::from_db_sz_simplepir_with_config(
                    rows,
                    item_bits,
                    YPIRSPConfig::for_poly_len(poly_len),
                );
                let ((pqr, pub_params), _) = client.generate_query_simplepir(3);
                assert_eq!(pqr.len(), query_words);
                assert_eq!(pub_params.len(), parameter_words);
                let body = serialize_ypir_query(pqr.as_slice(), pub_params.as_slice());
                let (parsed_pqr, parsed_params) = parse_query(&body, client.params()).unwrap();
                assert_eq!(parsed_pqr.len(), pqr.len() * U64_BYTES);
                assert_eq!(parsed_params.len(), pub_params.len() * U64_BYTES);
                assert_eq!(parsed_pqr, &body[U64_BYTES..U64_BYTES + parsed_pqr.len()]);
                assert_eq!(parsed_params, &body[U64_BYTES + parsed_pqr.len()..]);
            }
        }
    }

    #[test]
    fn rejects_wrong_section_sizes_for_both_degrees() {
        for (poly_len, parameter_words, matrix_words) in
            [(2048, 67_584, 6144), (4096, 196_608, 16_384)]
        {
            let params = params_for_scenario_simplepir_with_config(
                4096,
                98_304,
                YPIRSPConfig::for_poly_len(poly_len),
            );
            let pqr = vec![0; 4096];
            let pub_params = vec![0; parameter_words];

            for words in [0, 1, pqr.len() - 1, pqr.len() + 1] {
                let body = serialize_ypir_query(&vec![0; words], pub_params.as_slice());
                let err = parse_query(&body, &params).unwrap_err().to_string();
                assert!(err.contains("packed query size mismatch"), "{err}");
            }
            for words in [
                0,
                1,
                2,
                matrix_words,
                pub_params.len() - matrix_words,
                pub_params.len() - 1,
                pub_params.len() + 1,
                pub_params.len() + matrix_words,
            ] {
                let body = serialize_ypir_query(pqr.as_slice(), &vec![0; words]);
                let err = parse_query(&body, &params).unwrap_err().to_string();
                assert!(err.contains("pub_params size mismatch"), "{err}");
            }
            let tiny = serialize_ypir_query(&[0], &[0]);
            assert!(parse_query(&tiny, &params).is_err());
        }
    }

    #[test]
    fn rejects_truncated_misaligned_and_misdeclared_queries() {
        let params =
            params_for_scenario_simplepir_with_config(4096, 98_304, YPIRSPConfig::degree_4096());
        let mut pqr = vec![0; 4096];
        let mut pub_params = vec![0; 196_608];
        pqr[4095] = 0x1122334455667788;
        pub_params[0] = 0x8877665544332211;
        let valid = serialize_ypir_query(pqr.as_slice(), pub_params.as_slice());
        assert_eq!(valid.len(), 1_605_640);
        let (parsed_pqr, parsed_params) = parse_query(&valid, &params).unwrap();
        assert_eq!(&parsed_pqr[32_760..], &pqr[4095].to_le_bytes());
        assert_eq!(&parsed_params[..U64_BYTES], &pub_params[0].to_le_bytes());

        for len in 0..U64_BYTES {
            assert!(parse_query(&valid[..len], &params).is_err());
        }
        for len in [
            U64_BYTES,
            U64_BYTES + pqr.len() * U64_BYTES - 1,
            valid.len() - 1,
        ] {
            assert!(parse_query(&valid[..len], &params).is_err());
        }
        for declared in [7, 1u64 << 32 | 32_768, u64::MAX, 32_760, 32_776] {
            let mut body = valid.clone();
            body[..U64_BYTES].copy_from_slice(&declared.to_le_bytes());
            assert!(parse_query(&body, &params).is_err(), "header {declared}");
        }
        let mut body = valid;
        body.push(0);
        assert!(parse_query(&body, &params).is_err());
    }
}
