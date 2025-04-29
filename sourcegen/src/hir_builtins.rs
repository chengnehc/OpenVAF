use indexmap::IndexSet;
use quote::{format_ident, quote};
use stdx::iter::multiunzip;
use stdx::SKIP_HOST_TESTS;

use crate::{add_preamble, ensure_file_contents, project_root, reformat, to_upper_snake_case};

/// Refer to:
/// - LRM 4.3 Built-in mathematical functions
/// - LRM 4.4 Signal access functions
const BUILTINS: [&str; 26] = [
    // Table 4-14
    "ln",
    // "ln1p", // added in LRM-2023, not supported
    "log",
    "exp",
    // "expm1" , // added in LRM-2023, not supported
    "sqrt",
    "min",
    "max",
    "abs",
    "pow",
    "floor",
    "ceil",
    // Table 4-15
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "hypot",
    "sinh",
    "cosh",
    "tanh",
    "asinh",
    "acosh",
    "atanh",
    // Table 4-16
    "flow",
    "potential",
];

/// Refer to:
/// - LRM 4.5
/// - Syntax 4-3
/// - Annex A.8.2
///
/// Analog operators are functions which operate on more than just the current value of their arguments.
/// Instead, they maintain their internal state and their output is a function of both the input and the internal
/// state
///
/// One special analog operator is the limexp() function, which is a version of the exp() function with
/// built-in limits to improve convergence.
const ANALOG_OPERATORS: [&str; 17] = [
    "ddt",
    "ddx",
    "idt",
    "idtmod",
    "absdelay",
    "transition",
    "slew",
    "last_crossing",
    "limexp",
    "laplace_nd",
    "laplace_np",
    "laplace_zd",
    "laplace_zp",
    "zi_zp",
    "zi_zd",
    "zi_np",
    "zi_nd",
];

const ANALOG_OPERATORS_SYSFUN: [&str; 1] = ["$limit"];

/// LRM 4.6 Analysis dependent functions
const ANALYSIS_FUNS: [&str; 6] =
    ["analysis", "ac_stim", "noise_table", "noise_table_log", "white_noise", "flicker_noise"];

/// LRM 9 System tasks and functions
const SYSFUNS: [&str; 81] = [
    // Table 9-1
    "$display",
    "$strobe",
    "$write",
    "$monitor",
    "$debug",
    // Table 9-2
    "$fclose",
    "$fopen",
    "$fdisplay",
    "$fwrite",
    "$fstrobe",
    "$fmonitor",
    "$fgets",
    "$fscanf",
    "$swrite",
    "$sformat",
    "$sscanf",
    "$rewind",
    "$fseek",
    "$ftell",
    "$fflush",
    "$ferror",
    "$feof",
    "$fdebug",
    // Table 9-4
    "$finish",
    "$stop",
    "$fatal",
    "$warning",
    "$error",
    "$info",
    // Table 9-7
    "$abstime",
    // Table 9-8
    /* added in LRM-2023, not supported
    "$bitstoreal",
    "$itor",
    "$realtobits",
    "$rtoi",
    */
    // Table 9-9
    "$test$plusargs",
    "$value$plusargs",
    // Table 9-10
    "$dist_chi_square",
    "$dist_exponential",
    "$dist_poisson",
    "$dist_uniform",
    "$dist_erlang",
    "$dist_normal",
    "$dist_t",
    "$random",
    "$arandom",
    "$rdist_chi_square",
    "$rdist_exponential",
    "$rdist_poisson",
    "$rdist_uniform",
    "$rdist_erlang",
    "$rdist_normal",
    "$rdist_t",
    // Table 9-11
    // All of these functions, except $clog2, are aliases of the
    // analog math operators described in LRM 4.3.1
    "$clog2",
    "$ln",
    "$log10",
    "$exp",
    "$sqrt",
    "$pow",
    "$floor",
    "$ceil",
    "$sin",
    "$cos",
    "$tan",
    "$asin",
    "$acos",
    "$atan",
    "$atan2",
    "$hypot",
    "$sinh",
    "$cosh",
    "$tanh",
    "$asinh",
    "$acosh",
    "$atanh",
    /* added in LRM-2023, not supported
    "$min",
    "$max",
    "$abs",
    */
    // Table 9-12
    "$temperature",
    "$vt",
    "$simparam",
    "$simparam$str",
    // Table 9-13
    "$simprobe",
    // Table 9-14
    "$discontinuity",
    "$bound_step",
    // Table 9-16
    "$param_given",
    "$port_connected",
    // Table 9-17
    "$analog_node_alias",
    "$analog_port_alias",
    // Table 9-18
    // "$table_model", // not supported
    // Table 9-19
    /* added in LRM-2023, not supported
    "$receiver_count",
    */
];

/// Table 9-15 Hierarchical parameter system functions
const PARAM_SYSFUNS: [&str; 6] = ["mfactor", "xposition", "yposition", "angle", "hflip", "vflip"];

const UNSUPPORTED: [&str; 50] = [
    "simprobe",
    "analog_node_alias",
    "analog_port_alias",
    "test_plusargs",
    "value_plusargs",
    "zi_nd",
    "zi_np",
    "zi_zd",
    "zi_zp",
    "laplace_nd",
    "laplace_np",
    "laplace_zd",
    "laplace_zp",
    "last_crossing",
    "slew",
    "transition",
    "fclose",
    "fopen",
    "fdisplay",
    "fwrite",
    "fstrobe",
    "fmonitor",
    "fgets",
    "fscanf",
    "swrite",
    "sformat",
    "sscanf",
    "rewind",
    "fseek",
    "ftell",
    "fflush",
    "ferror",
    "feof",
    "fdebug",
    "dist_chi_square",
    "dist_exponential",
    "dist_poisson",
    "dist_uniform",
    "dist_erlang",
    "dist_normal",
    "dist_t",
    "random",
    "arandom",
    "rdist_chi_square",
    "rdist_exponential",
    "rdist_poisson",
    "rdist_uniform",
    "rdist_erlang",
    "rdist_normal",
    "rdist_t",
];

#[test]
fn generate_builtins() {
    if SKIP_HOST_TESTS {
        return;
    }
    let iter = BUILTINS
        .into_iter()
        .chain(SYSFUNS)
        .chain(ANALOG_OPERATORS)
        .chain(ANALOG_OPERATORS_SYSFUN)
        .chain(ANALYSIS_FUNS)
        .map(|builtin| {
            let is_sysfun = builtin.starts_with('$');
            let variant =
                if is_sysfun { builtin[1..].replace('$', "_") } else { builtin.to_owned() };
            let ident = format_ident!("{}", variant);
            let prefix = if is_sysfun { format_ident!("sysfun") } else { format_ident!("kw") };
            (prefix, ident, variant)
        });

    let (types, idents, variants): (Vec<_>, Vec<_>, Vec<_>) = multiunzip(iter);

    let unique_variants: IndexSet<_, ahash::RandomState> = variants.iter().cloned().collect();
    let constants =
        unique_variants.iter().map(|variant| format_ident!("{}", to_upper_snake_case(variant)));
    let unique_variants = unique_variants.iter().map(|variant| format_ident!("{}", variant));
    let indices = (0..unique_variants.len()).map(|i| i as u8);
    let analog_operators = ANALOG_OPERATORS.into_iter().map(|op| format_ident!("{}", op));
    let analog_operators_sysfun =
        ANALOG_OPERATORS_SYSFUN.into_iter().map(|op| format_ident!("{}", &op[1..]));
    let analysis_funs = ANALYSIS_FUNS.into_iter().map(|op| format_ident!("{}", op));
    let unsupported = UNSUPPORTED.into_iter().map(|op| format_ident!("{}", op));

    let params = PARAM_SYSFUNS.map(|var| format_ident!("{}", var));
    let variants = variants.iter().map(|var| format_ident!("{}", var));

    let hir_def = quote! {
        #[derive(Eq, PartialEq, Copy, Clone, Hash, Debug)]
        #[allow(nonstandard_style,unreachable_pub)]
        #[repr(u8)]
        pub enum BuiltIn{
            #(#unique_variants = #indices),*
        }

        impl BuiltIn{
            #[allow(clippy::match_like_matches_macro)]
            pub fn is_analog_operator(self)->bool{
                match self{
                    #(BuiltIn::#analog_operators)|* =>true,
                    _ => false
                }
            }

            #[allow(clippy::match_like_matches_macro)]
            pub fn is_analog_operator_sysfun(self)->bool{
                match self{
                    #(BuiltIn::#analog_operators_sysfun)|* =>true,
                    _ => false
                }
            }

            #[allow(clippy::match_like_matches_macro)]
            pub fn is_analysis_fun(self)->bool{
                match self{
                    #(BuiltIn::#analysis_funs)|* =>true,
                    _ => false
                }
            }

            #[allow(clippy::match_like_matches_macro)]
            pub fn is_unsupported(self)->bool{
                match self{
                    #(BuiltIn::#unsupported)|* =>true,
                    _ => false
                }
            }
        }

        #[derive(Eq, PartialEq, Copy, Clone, Hash, Debug)]
        #[allow(nonstandard_style,unreachable_pub)]
        pub enum ParamSysFun{
            #(#params),*
        }

        impl ParamSysFun{
            pub fn iter() -> impl Iterator<Item=Self> {
                [#(Self::#params),*].into_iter()
            }

            pub fn default_value(self) -> f64 {
                match self {
                    ParamSysFun::vflip | ParamSysFun::hflip | ParamSysFun::mfactor => 1f64,
                    ParamSysFun::xposition | ParamSysFun::yposition | ParamSysFun::angle => 0f64,
                }
            }
        }

        pub fn insert_builtin_def(dst: &mut IndexMap<Name, ScopeItemDef, RandomState>){
            #(dst.insert(#types::#idents, BuiltIn::#variants.into());)*
        }

        pub fn insert_param_sysfun(dst: &mut IndexMap<Name, ScopeItemDef, RandomState>){
            #(dst.insert(sysfun::#params, ParamSysFun::#params.into());)*
        }
    };

    let header = "use ahash::RandomState;
        use indexmap::IndexMap;
        use syntax::name::{kw, sysfun, Name};

        use crate::nameres::ScopeItemDef;
    ";
    let hir_def = format!("{}\n{}", header, hir_def);
    let hir_def = add_preamble("sourcegen::hir_builtins::generate_builtins()", reformat(hir_def));
    let file = project_root().join("openvaf").join("hir_def").join("src").join("builtin.rs");
    ensure_file_contents(&file, &hir_def);

    let const_cnt = constants.len();
    let hir_ty = quote! {
        const BUILTIN_INFO: [BuiltinInfo; #const_cnt] = [#(#constants),*];
        pub(crate) fn builtin_info(builtin: BuiltIn) -> BuiltinInfo{
            BUILTIN_INFO[builtin as u8 as usize]
        }
    };
    let header = "use hir_def::BuiltIn;

        use crate::builtin::*;
    ";
    let hir_ty = format!("{}\n{}", header, hir_ty);
    let hir_ty = add_preamble("sourcegen::hir_builtins::generate_builtins()", reformat(hir_ty));
    let file = project_root()
        .join("openvaf")
        .join("hir_ty")
        .join("src")
        .join("builtin")
        .join("generated.rs");
    ensure_file_contents(&file, &hir_ty);
}
