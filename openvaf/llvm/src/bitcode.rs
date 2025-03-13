use libc::{c_char, size_t};

use crate::{Bool, Context, MemoryBuffer, Module /*Value*/};

extern "C" {
    /* Memory Buffers */
    pub fn LLVMCreateMemoryBufferWithMemoryRange(
        input_data: *const c_char,
        input_data_len: size_t,
        buffer_name: *const c_char,
        requires_null_term: Bool,
    ) -> &'static MemoryBuffer;

    /* Bit Reader */
    pub fn LLVMParseBitcodeInContext2<'a>(
        ctx: &'a Context,
        buf: &MemoryBuffer,
        dst_module: &mut Option<&'a Module>,
    ) -> Bool;
}
