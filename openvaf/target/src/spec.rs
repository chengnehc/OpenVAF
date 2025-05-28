use std::collections::BTreeMap;

mod apple_base;
mod linux_base;
mod windows_msvc_base;

mod linker_flavor;
pub use linker_flavor::LinkerFlavor;

use crate::host_triple;

/// Everything `openvaf` knows about how to compile for a specific target.
///
/// Every field here must be specified, and has no default value.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Target {
    /// Target triple to pass to LLVM.
    pub llvm_target: String,
    /// String to use as the `target_pointer_width` `cfg` variable.
    pub pointer_width: u32,
    /// Architecture name for ABI considerations. Valid options include: "x86",
    /// "x86_64", "arm", "aarch64", "mips", "powerpc", "powerpc64", and others.
    pub arch: String,
    /// [Data layout](https://llvm.org/docs/LangRef.html#data-layout) to pass to LLVM.
    pub data_layout: String,
    /// Optional settings with defaults.
    pub options: TargetOptions,
}

pub type LinkArgs = BTreeMap<LinkerFlavor, Vec<String>>;

/// Optional aspects of target specification.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct TargetOptions {
    /// True if this is a built-in target
    pub is_builtin: bool,

    /// Default CPU to pass to LLVM. Corresponds to `llc -mcpu=$cpu`. Defaults to "generic".
    pub cpu: String,

    /// Default target features to pass to LLVM. These features will *always* be passed, and cannot
    /// be disabled even via `-C`. Corresponds to `llc -mattr=$features`.
    pub features: String,

    /// Default linker flavor used if `-C linker-flavor` or `-C linker` are not passed
    /// on the command line. Defaults to `LinkerFlavor::Ld`.
    pub linker_flavor: LinkerFlavor,

    /// Linker arguments that are passed *before* any user-defined libraries.
    pub pre_link_args: LinkArgs,

    /// Linker arguments that are unconditionally passed after any
    /// user-defined but before post-link objects. Standard platform
    /// libraries that should be always be linked to, usually go here.
    pub post_link_args: LinkArgs,

    /// On windows a manually generated importlib containing inline stdio definitions is required
    pub import_lib: &'static [u8],

    /// Whether the target toolchain is like Windows
    pub is_like_windows: bool,

    /// Whether the target toolchain is like macOS's.
    pub is_like_osx: bool,
}

impl Default for TargetOptions {
    fn default() -> Self {
        TargetOptions {
            is_builtin: true,
            cpu: "generic".to_string(),
            features: String::default(),
            linker_flavor: LinkerFlavor::Ld,
            pre_link_args: BTreeMap::default(),
            post_link_args: BTreeMap::default(),
            import_lib: &[],
            is_like_windows: false,
            is_like_osx: false,
        }
    }
}

macro_rules! supported_targets {
    ($(( $triple:literal, $module:ident ),)+) => {
        $( mod $module; )+

        const TARGETS: &[&str] = &[$($triple),+];

        pub fn supported_target_names() -> impl Iterator<Item = &'static str> {
            TARGETS.iter().copied()
        }

        pub fn supported_targets() -> impl Iterator<Item = Target> + Clone {
            [$( {
                let mut t = $module::target();
                t.options.is_builtin = true;
                t
            } ),*].into_iter()
        }

        fn load_specific(target: &str) -> Option<Target> {
            let mut t = match target {
                $( $triple => $module::target(), )+
                _ => return None,
            };
            t.options.is_builtin = true;
            Some(t)
        }
    }
}

supported_targets!(
    ("x86_64-unknown-linux", x86_64_unknown_linux),
    ("x86_64-pc-windows", x86_64_pc_windows),
    ("x86_64-apple-darwin", x86_64_apple_darwin),
    ("aarch64-unknown-linux", aarch64_unknown_linux),
    ("aarch64-pc-windows", aarch64_pc_windows),
    ("aarch64-apple-darwin", aarch64_apple_darwin),
);

impl Target {
    pub fn search(target_triple: &str) -> Option<Target> {
        load_specific(target_triple)
    }

    pub fn search_llvm_triple(target_triple: &str) -> Option<Target> {
        load_specific(target_triple.rsplit_once('-')?.0)
    }

    pub fn host_target() -> Option<Target> {
        Self::search(host_triple())
    }
}
