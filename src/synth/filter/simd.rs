//! SIMD helpers shared by the filters' lockstep kernels.

use std::arch::x86_64::*;

/// Transposes eight vectors of eight values: `output[j][i] == rows[i][j]`.
#[target_feature(enable = "avx")]
#[inline]
pub unsafe fn transpose8(rows: [__m256; 8]) -> [__m256; 8] {
    // Pairs of rows interleaved, then pairs of those, then the 128-bit halves swapped over
    let t0 = _mm256_unpacklo_ps(rows[0], rows[1]);
    let t1 = _mm256_unpackhi_ps(rows[0], rows[1]);
    let t2 = _mm256_unpacklo_ps(rows[2], rows[3]);
    let t3 = _mm256_unpackhi_ps(rows[2], rows[3]);
    let t4 = _mm256_unpacklo_ps(rows[4], rows[5]);
    let t5 = _mm256_unpackhi_ps(rows[4], rows[5]);
    let t6 = _mm256_unpacklo_ps(rows[6], rows[7]);
    let t7 = _mm256_unpackhi_ps(rows[6], rows[7]);
    let s0 = _mm256_shuffle_ps::<0x44>(t0, t2);
    let s1 = _mm256_shuffle_ps::<0xEE>(t0, t2);
    let s2 = _mm256_shuffle_ps::<0x44>(t1, t3);
    let s3 = _mm256_shuffle_ps::<0xEE>(t1, t3);
    let s4 = _mm256_shuffle_ps::<0x44>(t4, t6);
    let s5 = _mm256_shuffle_ps::<0xEE>(t4, t6);
    let s6 = _mm256_shuffle_ps::<0x44>(t5, t7);
    let s7 = _mm256_shuffle_ps::<0xEE>(t5, t7);
    [
        _mm256_permute2f128_ps::<0x20>(s0, s4),
        _mm256_permute2f128_ps::<0x20>(s1, s5),
        _mm256_permute2f128_ps::<0x20>(s2, s6),
        _mm256_permute2f128_ps::<0x20>(s3, s7),
        _mm256_permute2f128_ps::<0x31>(s0, s4),
        _mm256_permute2f128_ps::<0x31>(s1, s5),
        _mm256_permute2f128_ps::<0x31>(s2, s6),
        _mm256_permute2f128_ps::<0x31>(s3, s7),
    ]
}
