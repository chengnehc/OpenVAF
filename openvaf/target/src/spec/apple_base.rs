use crate::spec::{LinkerFlavor, TargetOptions};

pub fn opts() -> TargetOptions {
    TargetOptions {
        is_like_osx: true,
        linker_flavor: LinkerFlavor::Ld64,
        ..TargetOptions::default()
    }
}
