//! Fast ASCII whitespace scanning for hot text paths.
//!
//! The scanners process eight bytes per step with word-sized arithmetic. They
//! return the same answers as per-byte [`u8::is_ascii_whitespace`] loops.

/// One `1` per byte position.
const ONES: u64 = 0x0101_0101_0101_0101;
/// One `0x80` per byte position.
const HIGH: u64 = 0x8080_8080_8080_8080;
/// Low-bit mask that keeps additions from carrying across bytes.
const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;

/// Returns one `0x80` lane for each byte that is ASCII whitespace.
///
/// A lane matches when its byte equals `b' '` or lies in `b'\t'..=b'\r'`.
/// High bytes (`>= 0x80`) never match. The range test sets the high bit of
/// every lane before subtracting `0x09`, so no lane borrows from its
/// neighbour, and clears lanes whose low seven bits exceed `0x0D`.
#[inline]
fn ascii_whitespace_mask(word: u64) -> u64 {
    let ge_tab = (word | HIGH).wrapping_sub(0x0909_0909_0909_0909) & HIGH;
    let gt_cr = ((word & LOW7).wrapping_add(0x7272_7272_7272_7272) | word) & HIGH;
    let range = ge_tab & !gt_cr;
    let without_space = word ^ (ONES * 0x20);
    let space = without_space.wrapping_sub(ONES) & !without_space & HIGH;
    range | space
}

#[inline]
fn word_at(bytes: &[u8], offset: usize) -> u64 {
    let mut chunk = [0u8; 8];
    chunk.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(chunk)
}

/// Inputs shorter than this run a simple scalar loop. The word-at-a-time
/// scan only wins once its setup cost amortizes over several chunks.
const WORD_SCAN_MIN_LEN: usize = 32;

/// Index of the first ASCII whitespace byte, or `None`.
#[inline]
pub(crate) fn find_ascii_whitespace(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < WORD_SCAN_MIN_LEN {
        return bytes.iter().position(|byte| byte.is_ascii_whitespace());
    }
    let chunk_end = bytes.len() - bytes.len() % 8;
    let mut offset = 0;
    while offset < chunk_end {
        let mask = ascii_whitespace_mask(word_at(bytes, offset));
        if mask != 0 {
            return Some(offset + (mask.trailing_zeros() >> 3) as usize);
        }
        offset += 8;
    }
    bytes[offset..]
        .iter()
        .position(|byte| byte.is_ascii_whitespace())
        .map(|position| offset + position)
}

/// Index of the first byte that is not ASCII whitespace, or `None`.
#[inline]
pub(crate) fn find_non_ascii_whitespace(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < WORD_SCAN_MIN_LEN {
        return bytes.iter().position(|byte| !byte.is_ascii_whitespace());
    }
    let chunk_end = bytes.len() - bytes.len() % 8;
    let mut offset = 0;
    while offset < chunk_end {
        let mask = !ascii_whitespace_mask(word_at(bytes, offset)) & HIGH;
        if mask != 0 {
            return Some(offset + (mask.trailing_zeros() >> 3) as usize);
        }
        offset += 8;
    }
    bytes[offset..]
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map(|position| offset + position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_whitespace_like_scalar_scan() {
        let samples: Vec<Vec<u8>> = [
            &b""[..],
            b"a",
            b" ",
            b"\t\n\x0C\r x",
            b"abcdefgh",
            b"abcdefghi j",
            b"0123456789abcdef0123456789abcdefx y", // crosses the scalar cutoff
            &[
                0x80, 0x88, 0x89, 0x8D, 0xFF, 0x20, 0x09, 0x0A, 0x00, 0x08, 0x0E, 0x7F, b'a', b' ',
                b'\t', b'\r',
            ][..],
        ]
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect();

        for sample in &samples {
            assert_eq!(
                find_ascii_whitespace(sample),
                sample.iter().position(|byte| byte.is_ascii_whitespace()),
                "find_ascii_whitespace {sample:?}"
            );
            assert_eq!(
                find_non_ascii_whitespace(sample),
                sample.iter().position(|byte| !byte.is_ascii_whitespace()),
                "find_non_ascii_whitespace {sample:?}"
            );
        }

        // Lengths around the eight-byte chunk boundary and the scalar cutoff.
        for length in 0..40usize {
            let sample: Vec<u8> = (0..length)
                .map(|index| {
                    if index % 7 == 3 {
                        b' '
                    } else {
                        b'a' + index as u8
                    }
                })
                .collect();
            assert_eq!(
                find_ascii_whitespace(&sample),
                sample.iter().position(|byte| byte.is_ascii_whitespace()),
                "length {length}"
            );
            assert_eq!(
                find_non_ascii_whitespace(&sample),
                sample.iter().position(|byte| !byte.is_ascii_whitespace()),
                "length {length}"
            );
        }
    }
}
