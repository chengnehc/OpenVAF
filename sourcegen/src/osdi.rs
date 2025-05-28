use std::fmt::Write;
use std::{fs, mem};

use ahash::RandomState;
use indexmap::IndexMap;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote, ToTokens, TokenStreamExt};
use target::spec::supported_targets;

use crate::{add_preamble, ensure_file_contents, project_root, reformat, to_lower_snake_case};

#[test]
fn gen_osdi_structs() {
    let header_dir = project_root().join("openvaf").join("osdi").join("header");
    let headers: Vec<_> = fs::read_dir(header_dir)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            Header::new(entry)
        })
        .collect();

    let osdi_src_dir = project_root().join("openvaf").join("osdi").join("src").join("metadata");
    let osdi_test_dir = project_root().join("openvaf").join("openvaf").join("tests").join("load");
    let melange_src_dir = project_root().join("melange").join("core").join("src").join("veriloga");

    for header in &headers {
        let Header { version_major, version_minor, .. } = header;
        let semver = format!("{version_major}_{version_minor}");

        let targets = supported_targets().map(|target| target.llvm_target);
        let stdlibs = targets.clone().map(|target| format!("/stdlib_{semver}_{target}.bc"));
        let stdlib_idents: Vec<_> = targets
            .clone()
            .map(|target| {
                format_ident!("STDLIB_BITCODE_{}", target.to_uppercase().replace(['-', '.'], "_"))
            })
            .collect();
        let stdlib = quote! {
            #(const #stdlib_idents: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), #stdlibs));)*
            pub fn stdlib_bitcode(target: &target::spec::Target) -> &'static [u8]{
                match &*target.llvm_target {
                    #(#targets => #stdlib_idents,)*
                    triple => unreachable!("unknown target triple {triple}")
                }
            }
        };

        let res = HeaderParser { header, result: ParseResults::default(), offset: 0 }.parse();
        let consts = gen_defines(&res.defines);
        let tys = gen_llvm_tys(&res.tys);

        let file_header = "use mir_llvm::CodegenCx;\n";
        let file_string = format!("{file_header}\n{stdlib}\n{consts}\n\n{tys}");
        let file_string = add_preamble("gen_osdi_structs", reformat(file_string));
        let file_name = format!("osdi_{semver}.rs");
        ensure_file_contents(&osdi_src_dir.join(file_name), &file_string);

        let bindings = gen_bindings(&res.tys);
        let file_header = "use std::os::raw::{c_char, c_void};";
        let file_string = format!("{file_header}\n\n{consts}\n\n{bindings}");
        let file_string = add_preamble("gen_osdi_structs", reformat(file_string));
        let file_name = format!("osdi_{semver}.rs");
        ensure_file_contents(&osdi_test_dir.join(&file_name), &file_string);
        ensure_file_contents(&melange_src_dir.join(&file_name), &file_string);
    }
}

fn gen_defines(defines: &[(&str, &str)]) -> String {
    defines.iter().fold(String::new(), |mut output, (ident, val)| {
        let _ = write!(output, "pub const {ident}: u32 = {val};");
        output
    })
}

fn gen_llvm_tys<'a>(tys: &IndexMap<&'a str, OsdiStruct<'a>, RandomState>) -> String {
    let structs = tys.values().map(|it| OsdiStructInterp { info: it, lut: tys });
    let fields: Vec<_> =
        tys.values().map(|it| Ident::new(&it.llvm_ty_ident, Span::call_site())).collect();

    quote!(
        #(#structs)*

        #[derive(Clone)]
        pub struct OsdiTys<'ll>{
            #(pub #fields : &'ll llvm::Type),*
        }
        impl<'ll> OsdiTys<'ll>{
            pub fn new(ctx: &CodegenCx<'_, 'll>, target_data: &llvm::TargetData) -> Self{
                let mut builder = OsdiTyBuilder{
                    ctx,
                    target_data,
                    #(#fields: None),*
                };
                #(builder.#fields();)*
                builder.finish()
            }
        }
        struct OsdiTyBuilder<'a, 'b, 'll>{
            ctx: &'a CodegenCx<'b, 'll>,
            target_data: &'a llvm::TargetData,
            #(#fields : Option<&'ll llvm::Type>),*
        }
        impl<'ll> OsdiTyBuilder<'_, '_, 'll>{
            fn finish(self) -> OsdiTys<'ll>{
                OsdiTys{
                    #(#fields: self.#fields.unwrap()),*
                }
            }
        }
    )
    .to_string()
}

fn gen_bindings<'a>(tys: &IndexMap<&'a str, OsdiStruct<'a>, RandomState>) -> String {
    let tys = tys.iter().map(|(_, ty)| RustStruct(ty));
    quote!(#(#tys)*).to_string()
}

struct Header {
    version_major: u32,
    version_minor: u32,
    src: String,
}

impl Header {
    fn new(entry: fs::DirEntry) -> Option<Self> {
        if !entry.file_type().ok()?.is_file() {
            return None;
        }
        let name = entry.file_name();
        let semver = name.to_str()?.strip_prefix("osdi_")?.strip_suffix(".h")?;
        let (major, minor) = semver.split_once('_')?;
        let version_major = major.parse().unwrap();
        let version_minor = minor.parse().unwrap();

        let path = entry.path();
        let src = fs::read_to_string(path).ok()?;

        Some(Header { version_major, version_minor, src })
    }
}

struct HeaderParser<'a> {
    header: &'a Header,
    offset: usize,
    result: ParseResults<'a>,
}

#[derive(Default)]
struct ParseResults<'a> {
    tys: IndexMap<&'a str, OsdiStruct<'a>, RandomState>,
    defines: Vec<(&'a str, &'a str)>,
}

impl<'a> HeaderParser<'a> {
    fn parse(mut self) -> ParseResults<'a> {
        loop {
            let define_pos = self.src().find("#define");
            let typedef_pos = self.src().find("typedef");

            if let Some(pos) = define_pos {
                self.offset += pos;
                assert!(self.eat("#define"));
                self.parse_define();
                continue;
            }

            if let Some(pos) = typedef_pos {
                if define_pos.is_none_or(|define_pos| pos < define_pos) {
                    self.offset += pos;
                    assert!(self.eat("typedef"));
                    if self.eat("struct") {
                        self.parse_struct(false);
                    } else if self.eat("union") {
                        self.parse_struct(true);
                    }
                    continue;
                }
            }

            if typedef_pos.is_none() {
                break;
            }
        }

        self.result
    }

    fn parse_define(&mut self) {
        let ident = self.eat_ident().unwrap();
        let end = self.src().find('\n').unwrap_or_else(|| self.src().len());
        let val = self.src()[..end].trim();
        self.result.defines.push((ident, val));
    }

    fn parse_struct(&mut self, is_union: bool) {
        let ident = self.eat_ident().unwrap();
        assert!(self.eat("{"));

        let mut fields = Vec::new();
        while !self.eat("}") {
            let mut ty = self.parse_ty();
            let is_func_ptr = self.eat("(") && self.eat("*");
            let field_ident = self.eat_ident().unwrap();
            if is_func_ptr {
                assert!(self.eat(")"));
                assert!(self.eat("("));
                let mut args = Vec::new();
                while !self.eat(")") {
                    let ty = self.parse_ty();
                    let name = self.eat_ident().unwrap();
                    args.push((name, ty));
                    self.eat(",");
                }
                ty.func_args = Some(args);
            }

            self.eat(";");
            fields.push((field_ident, ty));
        }

        self.result.tys.insert(
            ident,
            OsdiStruct { is_union, ident, llvm_ty_ident: to_lower_snake_case(ident), fields },
        );
    }

    fn parse_ty(&mut self) -> Ty<'a> {
        let base_ty = match self.eat_ident().unwrap() {
            "double" => BaseTy::F64,
            "int" | "int32_t" => BaseTy::I32,
            "uint32_t" => BaseTy::U32,
            "size_t" => BaseTy::Usize,
            "char" => BaseTy::Char,
            "void" => BaseTy::Void,
            "bool" => BaseTy::Bool,
            name => BaseTy::Struct(name),
        };
        let mut indirection = 0;
        while self.eat("*") {
            indirection += 1;
        }

        Ty { base: base_ty, indirection, func_args: None }
    }

    fn eat(&mut self, kw: &str) -> bool {
        self.trim();
        if &self.src()[..kw.len()] == kw {
            self.offset += kw.len();
            true
        } else {
            false
        }
    }

    fn eat_ident(&mut self) -> Option<&'a str> {
        self.trim();
        let src = self.src();
        let is_ident_char = |c| matches!(c, '_'| 'a'..='z' | 'A' ..='Z' | '0' ..='9');
        let off = src.find(|c| !is_ident_char(c)).unwrap_or(src.len());
        if off != 0 {
            self.offset += off;
            Some(&src[..off])
        } else {
            None
        }
    }

    fn trim(&mut self) {
        let src = self.src();
        let (off, _) =
            src.char_indices().find(|(_, c)| !c.is_whitespace()).unwrap_or((src.len(), '\0'));
        self.offset += off;
    }

    fn src(&self) -> &'a str {
        &self.header.src[self.offset..]
    }
}

struct OsdiStruct<'a> {
    is_union: bool,
    ident: &'a str,
    llvm_ty_ident: String,
    fields: Vec<(&'a str, Ty<'a>)>,
}

struct OsdiStructInterp<'a, 'b> {
    info: &'b OsdiStruct<'a>,
    lut: &'b IndexMap<&'a str, OsdiStruct<'a>, RandomState>,
}

impl ToTokens for OsdiStructInterp<'_, '_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let OsdiStruct { is_union, ident, fields, llvm_ty_ident } = self.info;
        let llvm_ty_ident = Ident::new(llvm_ty_ident, Span::call_site());

        if !matches!(
            *ident,
            "OsdiInitError"
                | "OsdiSimParas"
                | "OsdiInitInfo"
                | "OsdiInitErrorPayload"
                | "OsdiSimInfo"
        ) {
            // assert!(!is_union, "union code generation is not implemented (yet)");
            let ident = Ident::new(ident, Span::call_site());
            let field_names = fields.iter().map(|(name, _)| Ident::new(name, Span::call_site()));
            let field_tys = fields.iter().map(|(_, ty)| TyInterpolater { ty, lut: self.lut });
            let field_ll_arrays = fields
                .iter()
                .enumerate()
                .map(|(pos, (name, ty))| LLVMValPreInterp { ty, name, pos: pos as u32 });
            let field_ll_vals = fields.iter().enumerate().map(|(pos, (name, ty))| LLVMValInterp {
                ty,
                name,
                pos: pos as u32,
                lut: self.lut,
            });
            let has_ll =
                fields.iter().any(|(_, ty)| ty.func_args.is_some() || ty.base == BaseTy::Void);
            let mut lt = quote!();
            let mut func_lt = quote!(<'ll>);
            if has_ll {
                mem::swap(&mut lt, &mut func_lt);
            }

            quote! {
                pub struct #ident #lt{
                    #(pub #field_names: #field_tys),*
                }
                impl #lt #ident #lt{
                    pub fn to_ll_val #func_lt (&self, ctx: &CodegenCx<'_,'ll>, tys: &'ll OsdiTys) -> &'ll llvm::Value{
                        #(#field_ll_arrays)*
                        let fields = [#(#field_ll_vals),*];
                        let ty = tys.#llvm_ty_ident;
                        ctx.const_struct(ty, &fields)
                    }
                }
            }
            .to_tokens(tokens);
        }

        let field_ll_tys: Vec<_> =
            fields.iter().map(|(_, ty)| LLVMTyInterp { ty, lut: self.lut }).collect();
        if *is_union {
            quote! {
                impl OsdiTyBuilder<'_, '_, '_>{
                    fn #llvm_ty_ident(&mut self){
                        let ctx = self.ctx;
                        unsafe{
                            let align = [#(llvm::LLVMABIAlignmentOfType(self.target_data, #field_ll_tys)),*].into_iter().max().unwrap();
                            let mut size = [#(llvm::LLVMABISizeOfType(self.target_data, #field_ll_tys)),*].into_iter().max().unwrap() as u32;
                            size = size.div_ceil(align);
                            let elem = ctx.ty_aint(align*8);
                            let ty = ctx.ty_array(elem, size);
                            self.#llvm_ty_ident = Some(ty);
                        }
                    }
                }
            }
            .to_tokens(tokens);
        } else {
            quote! {
                impl OsdiTyBuilder<'_, '_, '_>{
                    fn #llvm_ty_ident(&mut self){
                        let ctx = self.ctx;
                        let fields = [#(#field_ll_tys),*];
                        let ty = ctx.ty_struct(#ident, &fields);
                        self.#llvm_ty_ident = Some(ty);
                    }
                }
            }
            .to_tokens(tokens);
        }
    }
}

struct Ty<'a> {
    base: BaseTy<'a>,
    indirection: u32,
    func_args: Option<Vec<(&'a str, Ty<'a>)>>,
}

struct TyInterpolater<'b, 'a> {
    ty: &'b Ty<'a>,
    lut: &'b IndexMap<&'a str, OsdiStruct<'a>, RandomState>,
}
impl ToTokens for TyInterpolater<'_, '_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        if self.ty.func_args.is_some() || self.ty.base == BaseTy::Void {
            quote!(&'ll llvm::Value).to_tokens(tokens);
            return;
        }

        // let mut indirection = self.ty.indirection;
        // if self.ty.base == BaseTy::Char {
        //     indirection -= 1;
        // }

        BaseTyInterpolater { indirection: self.ty.indirection, base: self.ty.base, lut: self.lut }
            .to_tokens(tokens);
    }
}

struct BaseTyInterpolater<'b, 'a> {
    base: BaseTy<'a>,
    indirection: u32,
    lut: &'b IndexMap<&'a str, OsdiStruct<'a>, RandomState>,
}

impl ToTokens for BaseTyInterpolater<'_, '_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        if self.indirection == 0 || (self.indirection == 1 && self.base == BaseTy::Char) {
            self.base.to_tokens(tokens);
            if let BaseTy::Struct(name) = self.base {
                let ty = &self.lut[name];
                let has_ll = ty.fields.iter().any(|(_, ty)| ty.func_args.is_some());
                if has_ll {
                    quote!(<'ll>).to_tokens(tokens)
                }
            }
        } else {
            let next = BaseTyInterpolater { indirection: self.indirection - 1, ..*self };
            quote!(Vec<#next>).to_tokens(tokens)
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum BaseTy<'a> {
    F64,
    I32,
    U32,
    Usize,
    Char,
    Bool,
    Void,
    Struct(&'a str),
}

impl ToTokens for BaseTy<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = match self {
            BaseTy::Struct(name) => name,
            BaseTy::F64 => "f64",
            BaseTy::I32 => "i32",
            BaseTy::U32 => "u32",
            BaseTy::Usize => "usize",
            BaseTy::Bool => "bool",
            BaseTy::Void => "c_void",
            // C: char *  ->  Rust: String
            BaseTy::Char => {
                quote!(String).to_tokens(tokens);
                return;
            }
        };
        tokens.append(Ident::new(ident, Span::call_site()));
    }
}

struct LLVMTyInterp<'a, 'b> {
    ty: &'b Ty<'a>,
    lut: &'b IndexMap<&'a str, OsdiStruct<'a>, RandomState>,
}

impl ToTokens for LLVMTyInterp<'_, '_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let indirection = self.ty.indirection;
        let ty = if indirection == 0 && self.ty.func_args.is_none() {
            match self.ty.base {
                BaseTy::F64 => quote!(ctx.ty_double()),
                BaseTy::I32 | BaseTy::U32 => quote!(ctx.ty_int()),
                BaseTy::Usize => quote!(ctx.ty_size()),
                BaseTy::Char => quote!(ctx.ty_char()),
                BaseTy::Bool => quote!(ctx.ty_c_bool()),
                BaseTy::Void => quote!(ctx.ty_void()),
                BaseTy::Struct(ty) => {
                    let ty = &self.lut[ty].llvm_ty_ident;
                    let ty = Ident::new(ty, Span::call_site());
                    quote!(self.#ty.unwrap())
                }
            }
        } else {
            quote!(ctx.ty_ptr())
        };

        ty.to_tokens(tokens)

        // if let Some(_args) = &self.ty.func_args {
        // let args = args.iter().map(|(_, ty)| LLVMTyInterp { ty, lut: self.lut });
        // quote!(ctx.ty_ptr()).to_tokens(tokens)
        // } else {
        //     ty.to_tokens(tokens)
        // }
    }
}

struct LLVMValInterp<'a, 'b> {
    ty: &'b Ty<'a>,
    name: &'a str,
    pos: u32,
    lut: &'b IndexMap<&'a str, OsdiStruct<'a>, RandomState>,
}

impl ToTokens for LLVMValInterp<'_, '_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = Ident::new(self.name, Span::call_site());
        let src = quote!(self.#ident);
        if self.ty.func_args.is_some() || self.ty.base == BaseTy::Void {
            src.to_tokens(tokens);
            return;
        }
        let mut indirection = self.ty.indirection;
        if self.ty.base == BaseTy::Char {
            indirection -= 1;
        }
        if indirection != 0 {
            let base_ty = if indirection == 1 {
                match self.ty.base {
                    BaseTy::F64 => quote!(ctx.ty_double()),
                    BaseTy::I32 | BaseTy::U32 => quote!(ctx.ty_int()),
                    BaseTy::Usize => quote!(ctx.ty_size()),
                    BaseTy::Char => quote!(ctx.ty_ptr()),
                    BaseTy::Bool => quote!(ctx.ty_c_bool()),
                    BaseTy::Void => unreachable!(),
                    BaseTy::Struct(ty) => {
                        let ty = &self.lut[ty].llvm_ty_ident;
                        let ty = Ident::new(ty, Span::call_site());
                        quote!(tys.#ty)
                    }
                }
            } else {
                quote!(ctx.ty_ptr())
            };
            let pos = self.pos;
            let ident = format_ident!("arr_{pos}");
            quote!(ctx.const_arr_ptr(#base_ty, &#ident)).to_tokens(tokens);
            return;
        }

        let val = match self.ty.base {
            BaseTy::F64 => quote!(ctx.const_real(#src)),
            BaseTy::I32 => quote!(ctx.const_int(#src)),
            BaseTy::U32 => quote!(ctx.const_unsigned_int(#src)),
            BaseTy::Usize => quote!(ctx.const_usize(#src)),
            BaseTy::Char => quote!(ctx.const_str_uninterned(&#src)),
            BaseTy::Bool => quote!(ctx.const_c_bool(#src)),
            BaseTy::Void => unreachable!(),
            BaseTy::Struct(_) => {
                quote!(#src.to_ll_val(ctx, tys))
            }
        };

        val.to_tokens(tokens)
    }
}

struct LLVMValPreInterp<'a, 'b> {
    ty: &'b Ty<'a>,
    name: &'a str,
    pos: u32,
}

impl ToTokens for LLVMValPreInterp<'_, '_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = Ident::new(self.name, Span::call_site());
        let src = quote!(self.#ident);
        let mut indirection = self.ty.indirection;
        if self.ty.base == BaseTy::Char {
            indirection -= 1;
        }

        if indirection == 0 || self.ty.func_args.is_some() {
            return;
        };

        let calc_src = quote!(it);

        let val = match self.ty.base {
            BaseTy::F64 => quote!(ctx.const_real(*#calc_src)),
            BaseTy::I32 => quote!(ctx.const_int(*#calc_src)),
            BaseTy::U32 => quote!(ctx.const_unsigned_int(*#calc_src)),
            BaseTy::Usize => quote!(ctx.const_usize(*#calc_src)),
            BaseTy::Char => quote!(ctx.const_str_uninterned(#calc_src)),
            BaseTy::Bool => quote!(ctx.const_c_bool(*#calc_src)),
            BaseTy::Void if indirection == 1 => return,
            BaseTy::Void => unreachable!(),
            BaseTy::Struct(_) => {
                quote!(#calc_src.to_ll_val(ctx, tys))
            }
        };

        assert!(indirection <= 1);
        let pos = self.pos;
        let ident = format_ident!("arr_{pos}");
        quote! ( let #ident: Vec<_> = #src.iter().map(|it| #val).collect();).to_tokens(tokens)
    }
}

struct RustStruct<'a>(&'a OsdiStruct<'a>);
impl ToTokens for RustStruct<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let OsdiStruct { is_union, ident, ref fields, .. } = *self.0;
        let private =
            if matches!(ident, "OsdiDescriptor") { quote!(#[non_exhaustive]) } else { quote!() };
        let ident = format_ident!("{ident}");
        let kind = if is_union { quote!(union) } else { quote!(struct) };
        let field_names = fields.iter().map(|(name, _)| format_ident!("{name}"));
        let field_tys = fields.iter().map(|(_, ty)| RustTy(ty));
        quote! {
            #[repr(C)]
            #private
            pub #kind #ident {
                #(pub #field_names: #field_tys,)*
            }
        }
        .to_tokens(tokens);

        let funcs: Vec<_> = fields
            .iter()
            .filter_map(|(name, ty)| {
                Some(RustFunc {
                    name,
                    ret_ty: RustReturnTy(RustBasicTy {
                        base: ty.base,
                        indirection: ty.indirection,
                    }),
                    args: ty.func_args.as_ref()?,
                })
            })
            .collect();
        if !funcs.is_empty() {
            quote! {
                impl #ident{
                    #(#funcs)*
                }
            }
            .to_tokens(tokens)
        }
    }
}

struct RustBasicTy<'a> {
    base: BaseTy<'a>,
    indirection: u32,
}

impl ToTokens for RustBasicTy<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let RustBasicTy { base, indirection } = *self;
        let ident = match base {
            BaseTy::F64 => "f64",
            BaseTy::I32 => "i32",
            BaseTy::U32 => "u32",
            BaseTy::Usize => "usize",
            BaseTy::Bool => "bool",
            BaseTy::Char => "c_char",
            BaseTy::Void => "c_void",
            BaseTy::Struct(name) => name,
        };

        let base = Ident::new(ident, Span::call_site());
        let ptr = (0..indirection).map(|_| quote!(*mut));
        quote!(#(#ptr)* #base).to_tokens(tokens)
    }
}

struct RustReturnTy<'a>(RustBasicTy<'a>);

impl ToTokens for RustReturnTy<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ty = &self.0;
        if ty.indirection != 0 || ty.base != BaseTy::Void {
            quote!(-> #ty).to_tokens(tokens)
        }
    }
}
struct RustTy<'a>(&'a Ty<'a>);

impl ToTokens for RustTy<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let Ty { base, indirection, ref func_args } = *self.0;
        let base = RustBasicTy { base, indirection };
        match func_args {
            Some(args) => {
                let base = RustReturnTy(base);
                let arg_tys = args.iter().map(|(_, ty)| RustTy(ty));
                quote!(fn(#(#arg_tys),*) #base).to_tokens(tokens)
            }
            None => base.to_tokens(tokens),
        }
    }
}

struct RustFunc<'a> {
    name: &'a str,
    ret_ty: RustReturnTy<'a>,
    args: &'a [(&'a str, Ty<'a>)],
}

impl ToTokens for RustFunc<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let RustFunc { name, ret_ty, args } = self;
        let name = format_ident!("{name}");
        let arg_names = args.iter().map(|(name, _)| format_ident!("{name}"));
        let arg_tys = args.iter().map(|(_, ty)| RustTy(ty));
        let arg_name_refs = args.iter().map(|(name, _)| format_ident!("{name}"));
        quote! {
            pub fn #name(&self, #(#arg_names: #arg_tys),*) #ret_ty{
                (self.#name)(#(#arg_name_refs),*)
            }
        }
        .to_tokens(tokens)
    }
}
