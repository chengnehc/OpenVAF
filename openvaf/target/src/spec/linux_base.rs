use crate::spec::{LinkerFlavor, TargetOptions};

pub fn opts() -> TargetOptions {
    let mut opts = TargetOptions::default();

    let pre_link_args = opts.pre_link_args.entry(LinkerFlavor::Ld).or_default();
    let args = vec!["--no-add-needed".to_string(), "--hash-style=gnu".to_string()];
    pre_link_args.extend(args);

    opts
}
