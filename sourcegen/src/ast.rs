//! This module generates
//! 1. the `SyntaxKind` enum (in the `syntax_kind` module of `token` crate)
//! 2. newtype wrappers around `SyntaxToken` which implement trait `AstToken`.
//! 3. newtype wrappers around `SyntaxNode` which implement trait `AstNode`.
//!
//! See Also:
//!
//! https://github.com/rust-lang/rust/blob/master/src/tools/rust-analyzer/xtask/src/codegen/grammar.rs

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write;
use std::fs;

use proc_macro2::{Punct, Spacing};
use quote::{format_ident, quote};
use ungrammar::{Grammar, Rule};

use crate::{
    add_preamble, ensure_file_contents, pluralize, project_root, reformat, to_lower_snake_case,
    to_pascal_case, to_upper_snake_case,
};

mod src;
use self::src::{
    AstEnumSrc, AstEnumVariant, AstNodeSrc, AstSrc, Cardinality, Field, KindsSrc, KINDS_SRC,
    MANUAL_ENUMS,
};

#[test]
pub fn gen_ast() {
    let grammar = fs::read_to_string(project_root().join("openvaf/syntax/veriloga.ungram"))
        .unwrap()
        .parse()
        .unwrap();
    let ast_src = lower(&grammar);

    let ast_tokens_file = project_root().join("openvaf/syntax/src/ast/generated/tokens.rs");
    let contents = generate_tokens(&ast_src);
    ensure_file_contents(ast_tokens_file.as_path(), &contents);

    let ast_nodes_file = project_root().join("openvaf/syntax/src/ast/generated/nodes.rs");
    let contents = generate_nodes(&ast_src, KINDS_SRC);
    ensure_file_contents(ast_nodes_file.as_path(), &contents);

    let syntax_kinds_file = project_root().join("openvaf/tokens/src/syntax_kind/generated.rs");
    let syntax_kinds = generate_syntax_kinds(KINDS_SRC);
    ensure_file_contents(syntax_kinds_file.as_path(), &syntax_kinds);
}

fn generate_tokens(grammar: &AstSrc) -> String {
    let tokens = grammar.tokens.iter().map(|token| {
        let name = format_ident!("{}", token);
        let kind = format_ident!("{}", to_upper_snake_case(token));
        quote! {
            #[derive(Debug, Clone, PartialEq, Eq, Hash)]
            pub struct #name {
                pub(crate) syntax: SyntaxToken,
            }
            impl std::fmt::Display for #name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    std::fmt::Display::fmt(&self.syntax, f)
                }
            }
            impl AstToken for #name {
                fn can_cast(kind: SyntaxKind) -> bool { kind == #kind }
                fn cast(syntax: SyntaxToken) -> Option<Self> {
                    if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
                }
                fn syntax(&self) -> &SyntaxToken { &self.syntax }
            }
        }
    });

    add_preamble(
        "sourcegen/ast: generate_tokens()",
        reformat(
            quote! {
                use crate::SyntaxKind::{self, *};
                use crate::SyntaxToken;
                use crate::ast::AstToken;
                #(#tokens)*
            }
            .to_string(),
        ),
    )
    .replace("#[derive", "\n#[derive")
}

fn generate_nodes(grammar: &AstSrc, kinds: KindsSrc<'_>) -> String {
    let (node_defs, node_boilerplate_impls): (Vec<_>, Vec<_>) = grammar
        .nodes
        .iter()
        .map(|node| {
            let name = format_ident!("{}", node.name);
            let kind = format_ident!("{}", to_upper_snake_case(&node.name));
            let traits = node.traits.iter().map(|trait_name| {
                let trait_name = format_ident!("{}", trait_name);
                quote!(impl ast::#trait_name for #name {})
            });
            let methods = node.fields.iter().map(|field| {
                let method_name = field.method_name();
                let ty = field.ty();
                if field.is_many() {
                    quote! {
                        pub fn #method_name(&self) -> AstChildren<#ty> {
                            support::children(&self.syntax)
                        }
                    }
                } else if let Some(token_kind) = field.token_kind() {
                    quote! {
                        pub fn #method_name(&self) -> Option<#ty> {
                            support::token(&self.syntax, #token_kind)
                        }
                    }
                } else {
                    quote! {
                        pub fn #method_name(&self) -> Option<#ty> {
                            support::child(&self.syntax)
                        }
                    }
                }
            });
            (
                quote! {
                    #[pretty_doc_comment_placeholder_workaround]
                    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
                    pub struct #name {
                        pub(crate) syntax: SyntaxNode,
                    }
                    #(#traits)*
                    impl #name {
                        #(#methods)*
                    }
                },
                quote! {
                    impl AstNode for #name {
                        fn can_cast(kind: SyntaxKind) -> bool {
                            kind == #kind
                        }
                        fn cast(syntax: SyntaxNode) -> Option<Self> {
                            if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
                        }
                        fn syntax(&self) -> &SyntaxNode { &self.syntax }
                    }
                },
            )
        })
        .unzip();

    let (enum_defs, enum_boilerplate_impls): (Vec<_>, Vec<_>) = grammar
        .enums
        .iter()
        .map(|enm| {
            let name = format_ident!("{}", enm.name);
            let kinds: Vec<_> =
                enm.variants.iter().map(|var| format_ident!("{}", var.syntax_kind())).collect();
            let simple_variants = enm.variants.iter().map(|var| format_ident!("{}", var.name()));
            let variants: Vec<_> = simple_variants
                .clone()
                .chain(enm.nested_variant.as_ref().map(|it| format_ident!("{}", it)))
                .collect();
            let traits = enm.traits.iter().map(|trait_name| {
                let trait_name = format_ident!("{}", trait_name);
                quote!(impl ast::#trait_name for #name {})
            });
            let default_can_cast = enm.nested_variant.clone().map_or_else(
                || quote! {false},
                |default_case| {
                    let variant = format_ident!("{}", default_case);
                    quote! {#variant::can_cast(kind)}
                },
            );
            let default_cast = enm.nested_variant.clone().map_or_else(
                || quote! {return None},
                |default_case| {
                    let variant = format_ident!("{}", default_case);
                    quote! {#name::#variant(#variant::cast(syntax)?)}
                },
            );
            let nested_syntax = enm.nested_variant.clone().map(|n| {
                let variant = format_ident!("{}", n);
                quote! {#name::#variant(it) => it.syntax()}
            });
            let ast_node = if MANUAL_ENUMS.contains(&enm.name.as_str()) {
                quote!()
            } else {
                quote! {
                    impl AstNode for #name {
                        fn can_cast(kind: SyntaxKind) -> bool {
                            match kind {
                                #(#kinds)|* => true,
                                _ => #default_can_cast,
                            }
                        }
                        fn cast(syntax: SyntaxNode) -> Option<Self> {
                            let res = match syntax.kind() {
                                #(
                                #kinds => #name::#variants(#variants { syntax }),
                                )*
                                _ => #default_cast,
                            };
                            Some(res)
                        }
                        fn syntax(&self) -> &SyntaxNode {
                            match self {
                                #(
                                #name::#simple_variants(it) => &it.syntax,
                                )*
                                #nested_syntax
                            }
                        }
                    }
                }
            };
            (
                quote! {
                    #[pretty_doc_comment_placeholder_workaround]
                    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
                    pub enum #name {
                        #(#variants(#variants),)*
                    }
                    #(#traits)*
                },
                quote! {
                    #(
                    impl From<#variants> for #name {
                        fn from(node: #variants) -> #name {
                            #name::#variants(node)
                        }
                    }
                    )*
                    #ast_node
                },
            )
        })
        .unzip();

    let node_names = grammar.nodes.iter().map(|it| &it.name);
    let enum_names = grammar.enums.iter().map(|it| &it.name);
    let display_impls =
        enum_names.chain(node_names.clone()).map(|it| format_ident!("{}", it)).map(|name| {
            quote! {
                impl std::fmt::Display for #name {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        std::fmt::Display::fmt(self.syntax(), f)
                    }
                }
            }
        });

    let defined_nodes: HashSet<_> = node_names.collect();
    for node in kinds
        .nodes
        .iter()
        .map(|kind| to_pascal_case(kind))
        .filter(|name| !defined_nodes.contains(name))
    {
        eprintln!("Warning: node {} not defined in ast source", node);
        drop(node);
    }

    let ast = quote! {
        use crate::ast::{self, AstNode, AstChildren, support};
        use crate::SyntaxKind::{self, *};
        use crate::{SyntaxNode, SyntaxToken, T};

        #(#node_defs)*
        #(#enum_defs)*
        #(#node_boilerplate_impls)*
        #(#enum_boilerplate_impls)*
        #(#display_impls)*
    };
    let ast = ast.to_string().replace("T ! [", "T![");

    let mut res = String::with_capacity(ast.len() * 2);
    let mut docs =
        grammar.nodes.iter().map(|it| &it.doc).chain(grammar.enums.iter().map(|it| &it.doc));
    for chunk in ast.split("# [pretty_doc_comment_placeholder_workaround] ") {
        res.push_str(chunk);
        if let Some(doc) = docs.next() {
            write_doc_comment(doc, &mut res);
        }
    }

    add_preamble("sourcegen/ast: generate_nodes()", reformat(res)).replace("#[derive", "\n#[derive")
}

fn write_doc_comment(contents: &[String], dest: &mut String) {
    for line in contents {
        writeln!(dest, "///{}", line).unwrap();
    }
}

fn generate_syntax_kinds(grammar: KindsSrc<'_>) -> String {
    let (single_byte_tokens_values, single_byte_tokens): (Vec<_>, Vec<_>) = grammar
        .punct
        .iter()
        .filter(|(token, _name)| token.len() == 1)
        .map(|(token, name)| (token.chars().next().unwrap(), format_ident!("{}", name)))
        .unzip();

    let punctuation_values = grammar.punct.iter().map(|(token, _name)| {
        if "{}[]()".contains(token) {
            let c = token.chars().next().unwrap();
            quote! { #c }
        } else if *token == "'{" || *token == "(*" || *token == "*)" {
            quote! { #token }
        } else {
            let cs = token.chars().map(|c| Punct::new(c, Spacing::Joint));
            quote! { #(#cs)* }
        }
    });
    let punctuation =
        grammar.punct.iter().map(|(_token, name)| format_ident!("{}", name)).collect::<Vec<_>>();
    let punctuation_pretty: Vec<String> =
        grammar.punct.iter().map(|(token, _name)| format!("'{}'", token)).collect();

    let fmt_kw_as_variant = |&name| format_ident!("{}_KW", to_upper_snake_case(name));

    let all_keywords = grammar.keywords;
    let all_keywords_variants = all_keywords.iter().map(fmt_kw_as_variant).collect::<Vec<_>>();
    let all_keywords_tokens = all_keywords.iter().map(|kw| format_ident!("{}", kw));
    let keywords_pretty = grammar.keywords.iter().map(|kw| format!("'{}'", kw));

    let literals =
        grammar.literals.iter().map(|name| format_ident!("{}", name)).collect::<Vec<_>>();
    let tokens = grammar.tokens.iter().map(|name| format_ident!("{}", name)).collect::<Vec<_>>();
    let nodes = grammar.nodes.iter().map(|name| format_ident!("{}", name)).collect::<Vec<_>>();

    let ast = quote! {
        #![allow(bad_style, missing_docs, unreachable_pub)]
        /// The kind of syntax node, e.g. `IDENT`, `MODULE_KW`, or `PARAM_DECL`.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        #[repr(u16)]
        pub enum SyntaxKind {
            // Technical SyntaxKinds: they appear temporally during parsing,
            // but never end up in the final tree
            #[doc(hidden)]
            TOMBSTONE,
            #[doc(hidden)]
            EOF,
            #(#punctuation,)*
            #(#all_keywords_variants,)*
            #(#literals,)*
            #(#tokens,)*
            #(#nodes,)*

            // Technical kind so that we can cast from u16 safely
            #[doc(hidden)]
            __LAST,
        }
        use self::SyntaxKind::*;

        impl SyntaxKind {
            pub fn is_keyword(self) -> bool {
                match self {
                    #(#all_keywords_variants)|* => true,
                    _ => false,
                }
            }

            pub fn is_punct(self) -> bool {
                match self {
                    #(#punctuation)|* => true,
                    _ => false,
                }
            }

            pub fn is_literal(self) -> bool {
                match self {
                    #(#literals)|* => true,
                    _ => false,
                }
            }

            pub fn from_keyword(ident: &str) -> Option<SyntaxKind> {
                let kw = match ident {
                    #(#all_keywords => #all_keywords_variants,)*
                      "reg"
                    |"wreal"
                    |"wire"
                    |"uwire"
                    |"wand"
                    |"wor"
                    |"ground" => NET_TYPE,
                    _ => return None,
                };
                Some(kw)
            }

            pub fn from_char(c: char) -> Option<SyntaxKind> {
                let tok = match c {
                    #(#single_byte_tokens_values => #single_byte_tokens,)*
                    _ => return None,
                };
                Some(tok)
            }
        }

        impl std::fmt::Display for SyntaxKind {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let pretty = match self {
                    #(Self::#punctuation => #punctuation_pretty,)*
                    #(Self::#all_keywords_variants => #keywords_pretty,)*
                    Self::INT_NUMBER => "integer",
                    Self::STD_REAL_NUMBER | Self::SI_REAL_NUMBER  => "real number",
                    Self::STR_LIT => "string literal",
                    Self::LITERAL => "literal",
                    Self::IDENT | Self::NAME => "identifier",
                    Self::SYSFUN => "system function identifier",
                    Self::WHITESPACE => "whitespace",
                    Self::COMMENT => "comment",
                    Self::FUNCTION => "analog function decl.",
                    Self::PORT_DECL => "port decl.",
                    Self::NET_DECL => "net decl.",
                    Self::BRANCH_DECL => "branch decl.",
                    Self::VAR_DECL => "variable decl.",
                    Self::PARAM_DECL => "parameter decl.",
                    Self::ANALOG_BEHAVIOUR => "analog procedural block",
                    _ => return std::fmt::Debug::fmt(self, f)
                };
                write!(f, "{}", pretty)
            }
        }

        #[macro_export]
        macro_rules! T {
            #([#punctuation_values] => { $crate::SyntaxKind::#punctuation };)*
            #([#all_keywords_tokens] => { $crate::SyntaxKind::#all_keywords_variants };)*
            [ident] => { $crate::SyntaxKind::IDENT };
            [net_type] => { $crate::SyntaxKind::NET_TYPE };
            [sysfun] => { $crate::SyntaxKind::SYSFUN };
        }
    };

    add_preamble("sourcegen/ast: generate_syntax_kinds", reformat(ast.to_string()))
}

impl Field {
    fn is_many(&self) -> bool {
        matches!(self, Field::Node { cardinality: Cardinality::Many, .. })
    }
    fn token_kind(&self) -> Option<proc_macro2::TokenStream> {
        match self {
            Field::Token(token) => {
                let token: proc_macro2::TokenStream = token.parse().unwrap();
                Some(quote! { T![#token] })
            }
            _ => None,
        }
    }
    fn method_name(&self) -> proc_macro2::Ident {
        match self {
            Field::Token(name) => {
                let name = match name.as_str() {
                    ";" => "semicolon",
                    "'{'" => "l_curly",
                    "'}'" => "r_curly",
                    "'('" => "l_paren",
                    "')'" => "r_paren",
                    "'['" => "l_brack",
                    "']'" => "r_brack",
                    "<" => "l_angle",
                    ">" => "r_angle",
                    "=" => "eq",
                    "!" => "excl",
                    "*" => "star",
                    "&" => "amp",
                    "_" => "underscore",
                    "." => "dot",
                    "@" => "at",
                    ":" => "colon",
                    "#" => "pound",
                    "?" => "question_mark",
                    "," => "comma",
                    "|" => "pipe",
                    "\"(*\"" => "l_attr_paren",
                    "\"*)\"" => "r_attr_paren",
                    "\"'{\"" => "l_curly_arr",
                    "<+" => "contr",
                    _ => name,
                };
                let ident = format_ident!("{}_token", name);
                ident
            }
            Field::Node { name, .. } => {
                if name == "type" {
                    format_ident!("ty")
                } else {
                    format_ident!("{}", name)
                }
            }
        }
    }
    fn ty(&self) -> proc_macro2::Ident {
        match self {
            Field::Token(_) => format_ident!("SyntaxToken"),
            Field::Node { ty, .. } => format_ident!("{}", ty),
        }
    }
}

fn lower(grammar: &Grammar) -> AstSrc {
    let mut res = AstSrc {
        tokens: "Whitespace Comment StrLit IntNumber StdRealNumber SiRealNumber NetType Sysfun"
            .split_ascii_whitespace()
            .map(|it| it.to_string())
            .collect::<Vec<_>>(),
        nodes: vec![],
        enums: vec![],
    };

    for node in grammar.iter() {
        let name = grammar[node].name.clone();
        let rule = &grammar[node].rule;
        match lower_enum(grammar, rule) {
            Some((variants, nested_variant)) => {
                let enum_src = AstEnumSrc {
                    doc: Vec::new(),
                    name,
                    traits: Vec::new(),
                    variants,
                    nested_variant,
                };
                res.enums.push(enum_src);
            }
            None => {
                let mut fields = Vec::new();
                lower_rule(&mut fields, grammar, None, rule);
                res.nodes.push(AstNodeSrc { doc: Vec::new(), name, traits: Vec::new(), fields });
            }
        }
    }

    deduplicate_fields(&mut res);
    // extract_enums(&mut res);
    extract_struct_traits(&mut res);
    extract_enum_traits(&mut res);
    res
}

fn lower_enum(grammar: &Grammar, rule: &Rule) -> Option<(Vec<AstEnumVariant>, Option<String>)> {
    let Rule::Alt(alts) = rule else {
        return None;
    };
    let mut variants = Vec::new();
    let mut nested = None;
    let mut seen_non_token = false; // Don't generate enum when all alternatives are tokens
    for alt in alts {
        match alt {
            Rule::Node(it) if matches!(grammar[*it].rule, Rule::Alt(_)) => {
                assert!(nested.is_none(), "only a single nested enum is supported");
                seen_non_token = true;
                nested = Some(grammar[*it].name.clone());
            }
            Rule::Node(it) => {
                seen_non_token = true;
                variants.push(AstEnumVariant::Node(grammar[*it].name.clone()))
            }
            Rule::Token(it) => {
                variants.push(AstEnumVariant::Token(to_pascal_case(&grammar[*it].name)))
            }
            _ => return None,
        }
    }
    seen_non_token.then_some((variants, nested))
}

const MANUAL_LABEL: [&str; 16] = [
    // PrefixExpr, BinExpr, Assign
    "op",
    "lhs",
    "rhs",
    "lval",
    "rval",
    // SelectExpr
    "then_val",
    "else_val",
    // IfStmt
    "then_branch",
    "else_branch",
    // ForStmt
    "init",
    "incr",
    "for_body",
    // Range
    "start",
    "end",
    // ModulePort
    "kind",
    // EventStmt
    "sim_phases",
];

fn lower_rule(acc: &mut Vec<Field>, grammar: &Grammar, label: Option<&String>, rule: &Rule) {
    if lower_comma_list(acc, grammar, label, rule) {
        return;
    }

    match rule {
        Rule::Node(node) => {
            let ty = grammar[*node].name.clone();
            let name = label.cloned().unwrap_or_else(|| to_lower_snake_case(&ty));
            let field = Field::Node { name, ty, cardinality: Cardinality::Optional };
            acc.push(field);
        }
        Rule::Token(token) => {
            assert!(label.is_none(), "labels are only applicable to nodes");
            let mut name = grammar[*token].name.clone();
            if !matches!(&*name, "int_number" | "str_lit" | "std_real_number" | "si_real_number") {
                if "[]{}()".contains(&name) {
                    name = format!("'{}'", name);
                } else if name == "'{" || name == "(*" || name == "*)" {
                    name = format!("\"{}\"", name);
                };
                let field = Field::Token(name);
                acc.push(field);
            }
        }
        Rule::Rep(inner) => {
            let Rule::Node(node) = **inner else { panic!("unhandled rule: {:?}", rule) };
            let ty = grammar[node].name.clone();
            let name = label.cloned().unwrap_or_else(|| pluralize(&to_lower_snake_case(&ty)));
            let field = Field::Node { name, ty, cardinality: Cardinality::Many };
            acc.push(field);
        }
        Rule::Labeled { label: l, rule } => {
            assert!(label.is_none(), "double label (new label {})", l);
            if MANUAL_LABEL.contains(&l.as_str()) {
                return;
            }
            lower_rule(acc, grammar, Some(l), rule);
        }
        Rule::Seq(rules) | Rule::Alt(rules) => {
            for rule in rules {
                lower_rule(acc, grammar, label, rule)
            }
        }
        Rule::Opt(rule) => lower_rule(acc, grammar, label, rule),
    }
}

// (T (',' T)* ','?)
fn lower_comma_list(
    acc: &mut Vec<Field>,
    grammar: &Grammar,
    label: Option<&String>,
    rule: &Rule,
) -> bool {
    let Rule::Seq(rule) = rule else { return false };
    let [Rule::Node(node), Rule::Rep(repeat)] = rule.as_slice() else { return false };
    let Rule::Seq(repeat) = &**repeat else { return false };
    match repeat.as_slice() {
        [_comma, Rule::Node(n)] if n == node => (),
        _ => return false,
    }
    let ty = grammar[*node].name.clone();
    let name = label.cloned().unwrap_or_else(|| pluralize(&to_lower_snake_case(&ty)));
    let field = Field::Node { name, ty, cardinality: Cardinality::Many };
    acc.push(field);

    true
}

fn deduplicate_fields(ast: &mut AstSrc) {
    for node in &mut ast.nodes {
        let mut i = 0;
        'outer: while i < node.fields.len() {
            for j in 0..i {
                let f1 = &node.fields[i];
                let f2 = &node.fields[j];
                if f1 == f2 {
                    node.fields.remove(i);
                    continue 'outer;
                }
            }
            i += 1;
        }
    }
}

// fn extract_enums(ast: &mut AstSrc) {
//     for node in &mut ast.nodes {
//         for enm in &ast.enums {
//             let mut to_remove = Vec::new();
//             for (i, field) in node.fields.iter().enumerate() {
//                 let ty = field.ty().to_string();
//                 if enm.variants.iter().any(|it| it.name() == &ty) {
//                     to_remove.push(i);
//                 }
//             }
//             if to_remove.len() == enm.variants.len() {
//                 println!("{} is enum {}: {:?} == {:?}",&node.name,enm.name,enm.variants,&to_remove);
//                 node.remove_field(to_remove);
//                 let ty = enm.name.clone();
//                 let name = to_lower_snake_case(&ty);
//                 node.fields.push(Field::Node { name, ty, cardinality: Cardinality::Optional });
//             }
//         }
//     }
// }

fn extract_struct_traits(ast: &mut AstSrc) {
    let traits: &[(&str, &[&str])] =
        &[("AttrsOwner", &["attr_lists"]), ("ArgListOwner", &["arg_list"])];

    for node in &mut ast.nodes {
        for (name, methods) in traits {
            extract_struct_trait(node, name, methods);
        }
    }
}

fn extract_struct_trait(node: &mut AstNodeSrc, trait_name: &str, methods: &[&str]) {
    let mut to_remove = Vec::new();
    for (i, field) in node.fields.iter().enumerate() {
        let method_name = field.method_name().to_string();
        if methods.iter().any(|&it| it == method_name) {
            to_remove.push(i);
        }
    }
    if to_remove.len() == methods.len() {
        node.traits.push(trait_name.to_string());
        node.remove_field(to_remove);
    }
}

fn extract_enum_traits(ast: &mut AstSrc) {
    for enm in &mut ast.enums {
        if MANUAL_ENUMS.contains(&enm.name.as_str()) {
            continue;
        }
        let nodes = &ast.nodes;
        let mut variant_traits = enm
            .variants
            .iter()
            .filter(|var| matches!(var, AstEnumVariant::Node(_)))
            .map(|var| {
                nodes
                    .iter()
                    .find(|it| it.name == var.name())
                    .unwrap_or_else(|| panic!("Node not found {}", var.name()))
            })
            .map(|node| node.traits.iter().cloned().collect::<BTreeSet<_>>());

        let Some(mut enum_traits) = variant_traits.next() else { continue };
        for traits in variant_traits {
            enum_traits = enum_traits.intersection(&traits).cloned().collect();
        }
        enm.traits = enum_traits.into_iter().collect();
    }
}

impl AstNodeSrc {
    fn remove_field(&mut self, to_remove: Vec<usize>) {
        to_remove.into_iter().rev().for_each(|idx| {
            self.fields.remove(idx);
        });
    }
}
