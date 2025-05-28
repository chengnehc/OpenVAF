use crate::spec::{LinkerFlavor, Target, TargetOptions};

pub fn target() -> Target {
    let mut opts = super::apple_base::opts();
    opts.cpu = "apple-a14".to_string();

    opts.pre_link_args.insert(
        LinkerFlavor::Ld64,
        vec![
            "-arch".to_string(),
            "arm64".to_string(),
            "-undefined".to_string(),
            "dynamic_lookup".to_string(),
        ],
    );

    Target {
        llvm_target: "arm64-apple-macosx11.0.0".to_string(),
        pointer_width: 64,
        arch: "aarch64".to_string(),
        data_layout: "e-m:o-i64:64-i128:128-n32:64-S128".to_string(),
        options: TargetOptions { ..opts },
    }
}
