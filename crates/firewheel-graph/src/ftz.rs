// The following code is from nih_plug:
// https://github.com/robbert-vdh/nih-plug/blob/28b149ec4d62757d0b448809148a0c3ca6e09a95/src/wrapper/util.rs
//
// ISC License:
//
// Copyright (c) 2022-2024 Robbert van der Helm
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
// REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY
// AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
// INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM
// LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR
// OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
// PERFORMANCE OF THIS SOFTWARE.

use core::marker::PhantomData;

/// The bit that controls flush-to-zero behavior for denormals in 32 and 64-bit floating point
/// numbers on x86 family architectures. Rust 1.75 deprecated the built in functions for controlling
/// these registers. As listed in section 10.2.3.3 (Flush-To-Zero), bit 15 of the MXCSR register
/// controls the FTZ behavior.
///
/// <https://cdrdv2-public.intel.com/843823/252046-sdm-change-document-1.pdf>
#[cfg(target_feature = "sse")]
const SSE_FTZ_BIT: u32 = 1 << 15;

/// The bit that controls flush-to-zero behavior for denormals in 32 and 64-bit floating point
/// numbers on AArch64.
///
/// <https://developer.arm.com/documentation/ddi0595/2021-06/AArch64-Registers/FPCR--Floating-point-Control-Register>
#[cfg(target_arch = "aarch64")]
const AARCH64_FTZ_BIT: u64 = 1 << 24;

/// Enable the CPU's Flush To Zero flag while this object is in scope. If the flag was not already
/// set, it will be restored to its old value when this gets dropped.
pub(crate) struct ScopedFtz {
    /// Whether FTZ should be disabled again, i.e. if FTZ was not enabled before.
    should_disable_again: bool,
    /// We can't directly implement !Send and !Sync, but this will do the same thing. This object
    /// affects the current thread's floating point registers, so it may only be dropped on the
    /// current thread.
    _send_sync_marker: PhantomData<*const ()>,
}

impl ScopedFtz {
    pub fn enable() -> Self {
        #[cfg(not(miri))]
        {
            #[cfg(target_feature = "sse")]
            {
                // Rust 1.75 deprecated `_mm_setcsr()` and `_MM_SET_FLUSH_ZERO_MODE()`, so this now
                // requires inline assembly. See sections 10.2.3 (MXCSR Control and Status Register)
                // and 10.2.3.3 (Flush-To-Zero) from this document for more details:
                //
                // <https://cdrdv2-public.intel.com/843823/252046-sdm-change-document-1.pdf>
                let mut mxcsr: u32 = 0;
                unsafe { std::arch::asm!("stmxcsr [{}]", in(reg) &mut mxcsr) };
                let should_disable_again = mxcsr & SSE_FTZ_BIT == 0;
                if should_disable_again {
                    unsafe { std::arch::asm!("ldmxcsr [{}]", in(reg) &(mxcsr | SSE_FTZ_BIT)) };
                }

                return Self {
                    should_disable_again,
                    _send_sync_marker: PhantomData,
                };
            }

            #[cfg(target_arch = "aarch64")]
            {
                // There are no convenient intrinsics to change the FTZ settings on AArch64, so this
                // requires inline assembly:
                // https://developer.arm.com/documentation/ddi0595/2021-06/AArch64-Registers/FPCR--Floating-point-Control-Register
                let mut fpcr: u64;
                unsafe { std::arch::asm!("mrs {}, fpcr", out(reg) fpcr) };

                let should_disable_again = fpcr & AARCH64_FTZ_BIT == 0;
                if should_disable_again {
                    unsafe { std::arch::asm!("msr fpcr, {}", in(reg) fpcr | AARCH64_FTZ_BIT) };
                }

                return Self {
                    should_disable_again,
                    _send_sync_marker: PhantomData,
                };
            }
        }

        #[allow(unreachable_code)] // This is only unreachable if on SSE or aarch64
        Self {
            should_disable_again: false,
            _send_sync_marker: PhantomData,
        }
    }
}

impl Drop for ScopedFtz {
    fn drop(&mut self) {
        #[cfg(not(miri))]
        if self.should_disable_again {
            #[cfg(target_feature = "sse")]
            {
                let mut mxcsr: u32 = 0;
                unsafe { std::arch::asm!("stmxcsr [{}]", in(reg) &mut mxcsr) };
                unsafe { std::arch::asm!("ldmxcsr [{}]", in(reg) &(mxcsr & !SSE_FTZ_BIT)) };
            }

            #[cfg(target_arch = "aarch64")]
            {
                let mut fpcr: u64;
                unsafe { std::arch::asm!("mrs {}, fpcr", out(reg) fpcr) };
                unsafe { std::arch::asm!("msr fpcr, {}", in(reg) fpcr & !AARCH64_FTZ_BIT) };
            }
        }
    }
}
