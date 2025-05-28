use crate::spec::{LinkerFlavor, Target};

use super::apple_base;

pub fn target() -> Target {
    let mut opts = apple_base::opts();
    opts.cpu = "core2".to_string();
    opts.pre_link_args.insert(
        LinkerFlavor::Ld64,
        vec![
            "-m64".to_string(),
            "-arch".to_string(),
            "x86_64".to_string(),
            "-undefined".to_string(),
            "dynamic_lookup".to_string(),
        ],
    );

    Target {
        llvm_target: "x86_64-apple-macosx10.15.0".to_string(),
        pointer_width: 64,
        arch: "x86_64".to_string(),
        data_layout: "e-m:o-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
            .to_string(),
        options: opts,
    }
}
