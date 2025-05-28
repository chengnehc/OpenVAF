use crate::spec::Target;

const UCRT_IMPORTLIB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ucrt_x64.lib"));

pub fn target() -> Target {
    let mut opts = super::windows_msvc_base::opts();
    opts.cpu = "x86-64".to_string();
    opts.import_lib = UCRT_IMPORTLIB;

    Target {
        llvm_target: "x86_64-pc-windows-msvc".to_string(),
        pointer_width: 64,
        arch: "x86_64".to_string(),
        data_layout: "e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
            .to_string(),
        options: opts,
    }
}
