//! SIMD-accelerated control-byte scanner for the VT fast-path.
//!
//! VT-streams are dominated by printable ASCII (e.g. `seq 1 10000`). The
//! slow-path state machine pays per-byte cost for every character; on
//! workloads without escape sequences that's pure overhead.
//!
//! This module answers one question very fast: *where is the first
//! control byte in this chunk?* If there is none, the caller can bulk-
//! copy the whole chunk as printable ASCII into the active block's row.
//! If there is one, the caller processes the printable prefix in bulk
//! and hands the rest to the full VT parser.
//!
//! "Control byte" here means any byte in the C0 range (0x00..=0x1F) or
//! DEL (0x7F). Bytes ≥ 0x80 are treated as printable — the VT state
//! machine handles UTF-8 multibyte sequences, which is a separate
//! concern from control-byte detection.
//!
//! # Runtime dispatch
//!
//! - x86_64 with AVX2: 32-byte SIMD scan per iteration.
//! - aarch64 with NEON: 16-byte SIMD scan per iteration.
//! - scalar fallback: portable byte-by-byte, still branchless.
//!
//! The public API is architecture-agnostic — callers just use
//! [`scan_control`], and the right implementation is selected at runtime
//! (x86) or compile-time (aarch64 always has NEON).

/// Result of scanning a chunk of bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanResult {
    /// No control bytes in the input. All bytes are printable (C0-free
    /// and not DEL). High bytes (≥ 0x80) count as printable here — they
    /// are UTF-8 payload, not control.
    AllPrintable,
    /// A control byte sits at `offset` (0-based from the start of the
    /// input slice). The caller should process `input[..offset]` as
    /// printable, then route `input[offset..]` through the slow-path
    /// VT parser.
    ControlAt { offset: usize },
}

/// Scan `input` for the first control byte. Picks the best available
/// SIMD implementation at runtime.
#[inline]
pub fn scan_control(input: &[u8]) -> ScanResult {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: the caller upheld by the runtime feature check.
            return unsafe { scan_control_avx2(input) };
        }
        return scan_control_scalar(input);
    }
    #[cfg(target_arch = "aarch64")]
    {
        // aarch64 targets always support NEON per the standard Rust ABI.
        // SAFETY: aarch64 always implies NEON.
        unsafe { scan_control_neon(input) }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        scan_control_scalar(input)
    }
}

/// Portable scalar fallback. Still used on x86_64 without AVX2 and on
/// platforms where SIMD isn't available.
#[inline]
pub fn scan_control_scalar(input: &[u8]) -> ScanResult {
    for (i, &b) in input.iter().enumerate() {
        if b < 0x20 || b == 0x7F {
            return ScanResult::ControlAt { offset: i };
        }
    }
    ScanResult::AllPrintable
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn scan_control_avx2(input: &[u8]) -> ScanResult {
    use std::arch::x86_64::*;

    // SAFETY: all intrinsics below are sound given the AVX2 feature gate
    // and the bounds checks on `input.len()`.
    unsafe {
        let threshold = _mm256_set1_epi8(0x20);
        let del = _mm256_set1_epi8(0x7F);

        let mut offset = 0usize;
        let len = input.len();
        let ptr = input.as_ptr();

        while offset + 32 <= len {
            let chunk = _mm256_loadu_si256(ptr.add(offset) as *const _);
            // Signed compare works because 0x00..=0x1F and 0x7F are non-negative
            // in signed-byte interpretation too. `threshold > chunk` catches
            // the C0 range.
            let lt_ctrl = _mm256_cmpgt_epi8(threshold, chunk);
            let eq_del = _mm256_cmpeq_epi8(chunk, del);
            let combined = _mm256_or_si256(lt_ctrl, eq_del);
            let mask = _mm256_movemask_epi8(combined) as u32;
            if mask != 0 {
                return ScanResult::ControlAt {
                    offset: offset + mask.trailing_zeros() as usize,
                };
            }
            offset += 32;
        }

        // Tail: scalar over the last <32 bytes.
        for i in offset..len {
            let b = *ptr.add(i);
            if b < 0x20 || b == 0x7F {
                return ScanResult::ControlAt { offset: i };
            }
        }
        ScanResult::AllPrintable
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn scan_control_neon(input: &[u8]) -> ScanResult {
    use std::arch::aarch64::*;

    // SAFETY: aarch64 always has NEON; loads are guarded by the bounds checks.
    unsafe {
        let threshold = vdupq_n_u8(0x20);
        let del = vdupq_n_u8(0x7F);

        let mut offset = 0usize;
        let len = input.len();
        let ptr = input.as_ptr();

        while offset + 16 <= len {
            let chunk = vld1q_u8(ptr.add(offset));
            // `chunk < 0x20` → set 0xFF per-lane
            let lt_ctrl = vcltq_u8(chunk, threshold);
            let eq_del = vceqq_u8(chunk, del);
            let combined = vorrq_u8(lt_ctrl, eq_del);

            // Reduce: is any lane set? Pairwise max down to a single u8.
            let p1 = vpmax_u8(vget_low_u8(combined), vget_high_u8(combined));
            let p2 = vpmax_u8(p1, p1);
            let p3 = vpmax_u8(p2, p2);
            let p4 = vpmax_u8(p3, p3);
            let any = vget_lane_u8(p4, 0);
            if any != 0 {
                // Scalar descent to find exact offset inside this 16-byte chunk.
                for i in 0..16 {
                    let b = *ptr.add(offset + i);
                    if b < 0x20 || b == 0x7F {
                        return ScanResult::ControlAt { offset: offset + i };
                    }
                }
            }
            offset += 16;
        }

        for i in offset..len {
            let b = *ptr.add(i);
            if b < 0x20 || b == 0x7F {
                return ScanResult::ControlAt { offset: i };
            }
        }
        ScanResult::AllPrintable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_all(bytes: &[u8]) -> ScanResult {
        // All three implementations must agree on every input.
        let s = scan_control_scalar(bytes);
        let picked = scan_control(bytes);
        assert_eq!(s, picked, "dispatch picked != scalar for {bytes:?}");
        picked
    }

    #[test]
    fn empty_input_is_all_printable() {
        assert_eq!(probe_all(&[]), ScanResult::AllPrintable);
    }

    #[test]
    fn printable_only_reports_all_printable() {
        let bytes: Vec<u8> = (0x20u8..=0x7Eu8).collect();
        assert_eq!(probe_all(&bytes), ScanResult::AllPrintable);
    }

    #[test]
    fn ascii_banner_is_all_printable() {
        let bytes = b"hello world, this is plain printable text 1234567890 !@#$%^&*()".to_vec();
        assert_eq!(probe_all(&bytes), ScanResult::AllPrintable);
    }

    #[test]
    fn esc_at_position_zero() {
        let bytes = [0x1Bu8, b'A', b'B', b'C'];
        assert_eq!(probe_all(&bytes), ScanResult::ControlAt { offset: 0 });
    }

    #[test]
    fn newline_detected() {
        let mut bytes = vec![b'a'; 100];
        bytes[50] = b'\n';
        assert_eq!(probe_all(&bytes), ScanResult::ControlAt { offset: 50 });
    }

    #[test]
    fn carriage_return_detected() {
        let mut bytes = vec![b'x'; 32];
        bytes[20] = b'\r';
        assert_eq!(probe_all(&bytes), ScanResult::ControlAt { offset: 20 });
    }

    #[test]
    fn tab_detected() {
        let mut bytes = vec![b'y'; 40];
        bytes[10] = b'\t';
        assert_eq!(probe_all(&bytes), ScanResult::ControlAt { offset: 10 });
    }

    #[test]
    fn del_detected() {
        let mut bytes = vec![b'z'; 40];
        bytes[30] = 0x7F;
        assert_eq!(probe_all(&bytes), ScanResult::ControlAt { offset: 30 });
    }

    #[test]
    fn high_bytes_are_printable() {
        // UTF-8 multibyte sequences stay printable from the scanner's POV.
        let bytes = vec![0xE2, 0x82, 0xAC, 0xC3, 0xA9]; // € é
        assert_eq!(probe_all(&bytes), ScanResult::AllPrintable);
    }

    #[test]
    fn cross_boundary_detection_first_chunk() {
        // Place a control byte near the end of the first 32-byte chunk.
        let mut bytes = vec![b'a'; 64];
        bytes[31] = b'\n';
        assert_eq!(probe_all(&bytes), ScanResult::ControlAt { offset: 31 });
    }

    #[test]
    fn cross_boundary_detection_second_chunk() {
        // Control byte in tail past the first 32-byte chunk.
        let mut bytes = vec![b'a'; 64];
        bytes[40] = b'\n';
        assert_eq!(probe_all(&bytes), ScanResult::ControlAt { offset: 40 });
    }

    #[test]
    fn tail_past_simd_width_is_scanned() {
        // 33 bytes: one full SIMD chunk + 1-byte tail with a control byte.
        let mut bytes = vec![b'a'; 33];
        bytes[32] = 0x1B;
        assert_eq!(probe_all(&bytes), ScanResult::ControlAt { offset: 32 });
    }

    #[test]
    fn every_byte_value_consistency() {
        // Exhaustively test every byte value at position 0 of a padded buffer.
        for candidate in 0u8..=255u8 {
            let mut bytes = [b'a'; 48];
            bytes[5] = candidate;
            let expected = if candidate < 0x20 || candidate == 0x7F {
                ScanResult::ControlAt { offset: 5 }
            } else {
                ScanResult::AllPrintable
            };
            assert_eq!(
                probe_all(&bytes),
                expected,
                "mismatch for candidate byte {candidate:#x}"
            );
        }
    }
}
