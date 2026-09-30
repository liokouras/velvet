use std::{collections::HashMap, error::Error, fs, path::{Path as path, PathBuf}};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{self, FnArg, Ident, Path, Signature, Type, UseTree, visit_mut::{self, VisitMut}};
use super::report_error;

pub type FuncMetaData = (Option<String>, Signature, Path); // (Option<selftype>, Func Sig, func's qualified path)

pub struct FuncEntry {
    pub(super) name: Ident,
    pub(super) path: Path, // full qualified path with 'crate::..'
    pub(super) args: Vec<TokenStream>,
    pub(super) ret: Option<TokenStream>,
    pub(super) has_selfarg: bool,
}

// visitor to collect spawnable function signatures in a file
// and re-write custom and imported types as their full qualified paths
struct FnVisitor {
    qualified_path: String,
    import_map: HashMap<String, String>,
    functions: Vec<(String, Signature)>, // (qualified path for function, func sig)
    methods: Vec<(String, Option<String>, Signature)>, // (qualified path for method, selftype, func sig)
    current_selftype: Option<String>,
    in_sig: bool,
    current_fn: String, // name of the spawnable whose signature is being visited (for error messages)
    collecting: bool, // first pass: only collect definitions/imports/modules (so that their order in the file does not matter)
}
impl VisitMut for FnVisitor {
    fn visit_item_mod_mut(&mut self, node: &mut syn::ItemMod) {
        // need to push this nested module name onto the path
        let mod_name = node.ident.to_string();
        let old_path = self.qualified_path.clone();
        // record the module itself, so that module-qualified types (`mod_name::Type`) can be resolved
        self.import_map.insert(mod_name.clone(), format!("{}{}", old_path, mod_name));
        self.qualified_path = format!("{}{}::", old_path, mod_name);

        // recurse into mod
        if let Some((_, items)) = &mut node.content {
            for item in items {
                self.visit_item_mut(item);
            }
        }

        // restore path
        self.qualified_path = old_path;
    }

    fn visit_item_enum_mut(&mut self, node: &mut syn::ItemEnum) {
        let enum_name = node.ident.to_string();
        let full_path = format!("{}{}", self.qualified_path, enum_name);
        self.import_map.insert(enum_name, full_path);
        visit_mut::visit_item_enum_mut(self, node);
    }

    fn visit_item_struct_mut(&mut self, node: &mut syn::ItemStruct) {
        let struct_name = node.ident.to_string();
        let full_path = format!("{}{}", self.qualified_path, struct_name);
        self.import_map.insert(struct_name, full_path);
        visit_mut::visit_item_struct_mut(self, node);
    }

    fn visit_item_type_mut(&mut self, node: &mut syn::ItemType) {
        let alias_name = node.ident.to_string();
        let full_path = format!("{}{}", self.qualified_path, alias_name);
        self.import_map.insert(alias_name, full_path);
        visit_mut::visit_item_type_mut(self, node);
    }

    fn visit_item_impl_mut(&mut self, node: &mut syn::ItemImpl) {
        // if we are in an 'impl' block, keep track of what the selftype is
        if let Type::Path(type_path) = &*node.self_ty {
            let selftype = type_path.path.segments.last().unwrap().ident.to_string();
            self.current_selftype = Some(selftype.clone());
            let full_path = format!("{}{}", self.qualified_path, selftype);
            // also record in import_map in case custom type is used 
            // TODO BAZI: what if type def is after first use..?
            self.import_map.insert(selftype, full_path);
        }

        // recurse [mostly in case there are use-statements]
        visit_mut::visit_item_impl_mut(self, node);

        // add all spawnable method signatures
        for item in &mut node.items {
            if let syn::ImplItem::Fn(method) = item {
                if is_spawnable(&method.attrs) && !self.collecting {
                    // re-write method signature with qualified types
                    self.current_fn = method.sig.ident.to_string();
                    self.in_sig = true;
                    for input in &mut method.sig.inputs {
                        self.visit_fn_arg_mut(input);
                    }
                    if let syn::ReturnType::Type(_, ref mut ret) = method.sig.output {
                        self.visit_type_mut(ret);
                    }
                    self.in_sig = false;

                    self.methods.push((self.qualified_path.clone(), self.current_selftype.clone(), method.sig.clone()));
                }
            }
        }
        
        // reset selftype after exiting impl block
        self.current_selftype = None;
    }

    fn visit_item_fn_mut(&mut self, node: &mut syn::ItemFn) {
        if is_spawnable(&node.attrs) && !self.collecting {
            // re-write function signature with qualified types
            self.current_fn = node.sig.ident.to_string();
            self.in_sig = true;
            for arg in &mut node.sig.inputs {
                self.visit_fn_arg_mut(arg);
            }
            if let syn::ReturnType::Type(_, ref mut ret) = node.sig.output {
                self.visit_type_mut(ret);
            }
            self.in_sig = false;

            // add function signature
            self.functions.push((self.qualified_path.clone(), node.sig.clone()));
        }
    }
    
    fn visit_item_use_mut(&mut self, node: &mut syn::ItemUse) {
        // collect list of imports
        self.collect_imports(&node.tree, String::new());
        visit_mut::visit_item_use_mut(self, node);
    }

    fn visit_type_path_mut(&mut self, node: &mut syn::TypePath) {
        // skip selftypes & only re-write signatures
        if is_self(node) || !self.in_sig { return; }

        // recurse first in case of nested types
        for seg in &mut node.path.segments {
            if let syn::PathArguments::AngleBracketed(ref mut angle_args) = seg.arguments {
                for arg in &mut angle_args.args {
                    if let syn::GenericArgument::Type(ty) = arg {
                        self.visit_type_mut(ty);
                    }
                }
            }
        }

        // re-write the type to its path from the crate root (the generated code lives at the crate root)
        if let Some(top_level) = node.path.segments.first() {
            let ident = &top_level.ident;
            let name = ident.to_string();
            if is_primitive(&name) && node.path.segments.len() == 1 {
                return; // do not rewrite primitives (and prelude types)
            }

            // paths that are already valid at the crate root: `crate::..`, `std::..`, `core::..`, `alloc::..`, `::..`
            if node.path.leading_colon.is_some() || matches!(name.as_str(), "crate" | "std" | "core" | "alloc") {
                return;
            }

            // paths relative to the current module: `self::..`, `super::..`
            if name == "self" || name == "super" {
                let num_relative = node.path.segments.iter().take_while(|seg| seg.ident == "self" || seg.ident == "super").count();
                let prefix: Vec<String> = node.path.segments.iter().take(num_relative).map(|seg| seg.ident.to_string()).collect();
                if let Some(module) = self.relative_module(&prefix) {
                    let mut full_path: Path = syn::parse_str(&module).expect(&format!("could not parse {}", module));
                    full_path.segments.extend(node.path.segments.iter().skip(num_relative).cloned());
                    node.path = full_path;
                }
                return; // (too many `super`s: left as written; rustc reports it)
            }

            if let Some(full_path_str) = self.import_map.get(&name) {
                // parse the full path string into a Path
                let mut full_path: Path = syn::parse_str(full_path_str).expect(&format!("could not parse {}",full_path_str));

                if node.path.segments.len() == 1 {
                    // `Type<..>`: re-attach the generic arguments to the fully qualified path
                    if let Some(last_seg) = full_path.segments.last_mut() {
                        last_seg.arguments = top_level.arguments.clone();
                    }
                } else {
                    // `module::Type<..>` with a known module (or imported name): keep the rest of the path
                    full_path.segments.extend(node.path.segments.iter().skip(1).cloned());
                }

                // overwrite
                node.path = full_path;
                return;
            }

            // a path starting with an unknown name, e.g. an external crate (`rand::rngs::StdRng`): valid at the crate root as written
            if node.path.segments.len() > 1 {
                return;
            }

            report_error(&format!(
                "spawnable `{}`: cannot resolve type `{}` in its signature: it is not defined in this file and not imported with `use`. \
                 Velvet generates code at the crate root, so it needs to know where the type lives: \
                 import it (e.g. `use crate::module::{};`) or write its path from the crate root (e.g. `crate::module::{}`).",
                self.current_fn, name, name, name));
        }
    }
}
impl FnVisitor {
    // the module (e.g. `crate::a`) that a `self::`/`super::` prefix refers to, from the current module;
    // None if there are more `super`s than enclosing modules
    fn relative_module(&self, prefix: &[String]) -> Option<String> {
        let mut module: Vec<&str> = self.qualified_path.trim_end_matches("::").split("::").collect();
        for seg in prefix {
            if seg == "super" {
                if module.len() <= 1 { return None; }
                module.pop();
            }
        }
        Some(module.join("::"))
    }

    // an imported path, resolved to the crate root if it starts with `self::`/`super::`
    fn resolve_import(&self, path: String) -> String {
        let segments: Vec<String> = path.split("::").map(|s| s.to_string()).collect();
        let num_relative = segments.iter().take_while(|s| *s == "self" || *s == "super").count();
        if num_relative == 0 { return path; }
        match self.relative_module(&segments[..num_relative]) {
            Some(module) => std::iter::once(module).chain(segments[num_relative..].iter().cloned()).collect::<Vec<_>>().join("::"),
            None => path,
        }
    }

    fn collect_imports(&mut self, use_tree: &UseTree, prefix: String) {
        match use_tree {
            UseTree::Path(syn::UsePath { ident, tree, .. }) => {
                let new_prefix = if prefix.is_empty() {
                    ident.to_string()
                } else {
                    format!("{prefix}::{}", ident)
                };
                self.collect_imports(tree, new_prefix);
            }
            UseTree::Name(syn::UseName { ident }) => {
                let full_path = if prefix.is_empty() {
                    ident.to_string()
                } else {
                    format!("{prefix}::{}", ident)
                };
                let full_path = self.resolve_import(full_path);
                self.import_map.insert(ident.to_string(), full_path);
            }
            UseTree::Rename(syn::UseRename { ident, rename, .. }) => {
                let full_path = if prefix.is_empty() {
                    ident.to_string()
                } else {
                    format!("{prefix}::{}", ident)
                };
                let full_path = self.resolve_import(full_path);
                self.import_map.insert(rename.to_string(), full_path);
            }
            UseTree::Group(syn::UseGroup { items, .. }) => {
                for item in items {
                    self.collect_imports(item, prefix.clone());
                }
            }
            _ => {}
        }
    }
}

// helpers for the visitor..
fn is_primitive(ident: &str) -> bool {
    matches!(ident,
        "bool" | "char" | "str" |
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" |
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" |
        "f32" | "f64" | "String" | "Vec" | "Box" | "Option" | "Result")
}
fn is_spawnable(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("spawnable"))
}
fn is_self(ty: &syn::TypePath) -> bool {
    // qualified path like `<Self as Trait>::Assoc`
    if let Some(q) = &ty.qself {
        if let Type::Path(inner) = &*q.ty {
            if inner.qself.is_none() && inner.path.segments.len() == 1 && inner.path.segments[0].ident == "Self" {
                return true;
            }
        }
    }

    // bare `Self`
    ty.qself.is_none() && ty.path.segments.len() == 1 && ty.path.segments[0].ident == "Self"
}

/*
    input: vector of filepaths for files containing spawnable functions
    output: a vector of tuples with:
            1. Option<selftype>, in case the spawnable function is an associated method,
            2. the spawnable function signature, where types are potentially modified to have full qualified paths
            3. the spawnable function's full qualified path
*/
pub(crate) fn find_functions(filepaths: Vec<PathBuf>) -> Vec<FuncMetaData> {
    let mut all_funcs: Vec<Vec<FuncMetaData>> = Vec::new();

    for filepath in filepaths {
        match get_funcs(&filepath) {
            Ok(funcs) => { all_funcs.push(funcs); }
            Err(e) => {
                report_error(&format!("velvet::generate: could not read or parse file {:?} while looking for spawnable functions: {}", filepath, e));
            }
        }
    }

    return all_funcs.into_iter().flatten().collect();
}

/*
    input: rust filepath and list of functions to search for
    output: Vector of corresponding function Option<selftype>-signature-qualified path tuples
    error: io error if cannot open file
*/
fn get_funcs(filepath: &path) -> Result<Vec<FuncMetaData>, Box<dyn Error>> {
    let qualified_path = get_qualified_path(&filepath).expect(&format!("could not extract qualified path from {:?}", filepath));

    let file_content = fs::read_to_string(filepath)?;
    let mut ast = syn::parse_file(&file_content)?;
    let mut funcs = FnVisitor { 
        qualified_path: qualified_path.clone(),
        import_map: HashMap::new(),
        functions: Vec::new(), 
        methods: Vec::new(), 
        current_selftype: None, 
        in_sig: false,
        current_fn: String::new(),
        collecting: true,
    };
    // first pass: collect all definitions, imports and modules of the file; second pass: re-write the signatures
    funcs.visit_file_mut(&mut ast.clone());
    funcs.collecting = false;
    funcs.visit_file_mut(&mut ast);
    
    //  convert collected function- and method-info into FuncMetaData type
    let mut spawnables = Vec::new();
    for (qualified_path, sig) in funcs.functions {
        let func_name = sig.ident.to_string();
        let full_func_name = format!("{}{}", qualified_path, func_name);
        spawnables.push((None, sig, syn::parse_str::<Path>(&full_func_name).expect(&format!("Failed to parse function path: {}", full_func_name))));
    }
    for (qualified_path, selftype, sig) in funcs.methods {
        let method_name = sig.ident.to_string();
        let selftype = selftype.unwrap();
        let full_method_name = format!("{}{}::{}", qualified_path, selftype, method_name);
        let selftype = format!("{}{}", qualified_path, selftype);
        spawnables.push((Some(selftype), sig, syn::parse_str::<Path>(&full_method_name).expect(&format!("Failed to parse method path: {}", full_method_name))));
    }

    Ok(spawnables)
}

/*
    re-writes a filepath like src/some_module/some_file.rs
    to crate::some_module::some_file::
*/
fn get_qualified_path(filepath: &path) -> Option<String> {
    // strip "src/"
    let src_prefix = path::new("src");
    let rel_path = filepath.strip_prefix(src_prefix).ok()?;

    // handle root files (main.rs, lib.rs)
    if rel_path == path::new("main.rs") || rel_path == path::new("lib.rs") {
        return Some("crate::".to_string());
    }

    let mut components: Vec<String> = rel_path
        .iter()
        .map(|os_str| os_str.to_string_lossy().to_string())
        .collect();
    let last = components.pop()?;

    let module_name = if last == "mod.rs" {
        // mod.rs means the folder itself is the module, so just use components
        components.join("::")
    } else if last.ends_with(".rs") {
        // remove .rs extension
        let last = last.trim_end_matches(".rs");
        components.push(last.to_string());
        components.join("::")
    } else {
        // if no .rs extension, just join
        components.push(last);
        components.join("::")
    };

    Some(format!("crate::{}::", module_name))
}

/*
    reformat the FuncMetaData into a 'database' of TokenStreams to be used by the quotes module
    essentially: 
        - parses the signature into the components relevant for the quotes module
        - renames any self-parameters to their full qualified type
        - wraps return type in an Option
*/
pub fn build_funcs_db(funcs: Vec<FuncMetaData> ) -> Vec<FuncEntry> {
    let mut database = Vec::new();
    for (selftype, sig, path) in funcs.into_iter() {
        let has_selfarg = sig.receiver().is_some();
        let func_name = sig.ident;

        // qualified name of the spawnable, for error messages, e.g. `crate::matrix_par::Matrix::spawn_matmul`
        let display_path = path.segments.iter().map(|seg| seg.ident.to_string()).collect::<Vec<_>>().join("::");

        let arg_types: Vec<_> = sig.inputs.iter().map(|arg| {
            match arg {
                FnArg::Typed(pat_type) => {
                    let ty = &*pat_type.ty;
                    if is_non_static_reference(ty) {
                        let pat = &pat_type.pat;
                        report_error(&format!(
                            "spawnable `{}`: argument `{}` has type `{}`, a reference without `'static` lifetime. \
                             Spawned tasks may run on another thread, so arguments must be `Send + 'static`: \
                             pass owned data (e.g. `Vec<T>`, `Box<T>`), share it with `Arc<T>`, or use a `&'static` reference (e.g. from `Box::leak`).",
                            display_path, display(quote!(#pat)), display(quote!(#ty))));
                    }
                    quote!(#ty)
                },
                FnArg::Receiver(recv) => {
                    let selftype =  selftype.as_ref().unwrap();
                    let self_ty = syn::parse_str::<Type>(selftype).expect(&format!("Could not parse {} into a type", selftype));
                    // syn gives shorthand receivers their full type too: `self` is `Self`, `&self` is `&Self`,
                    // `&'static self` is `&'static Self`; typed receivers (`self: Arc<Self>`) have the written type.
                    // so both forms get the same check, and the frame stores exactly the receiver's type.
                    let recv_ty = &*recv.ty;
                    if is_non_static_reference(recv_ty) {
                        let written = if recv.colon_token.is_some() { format!("self: {}", display(quote!(#recv_ty))) } else { display(quote!(#recv)) };
                        let method = &func_name;
                        report_error(&format!(
                            "spawnable method `{}` takes `{}` as receiver, a reference without `'static` lifetime. \
                             Spawned tasks may run on another thread, so a spawnable method must own its receiver or borrow it for `'static`: \
                             use `self: Arc<Self>` to share read-only data (call it as `x.clone().{}(..)`), `self` or `self: Box<Self>` to move it into the task, \
                             or `&'static self` / `self: &'static Self`.",
                            display_path, written, method));
                    }
                    // fully qualified type, with the actual selftype instead of 'Self'
                    let mut modified_self = recv_ty.clone();
                    ReplaceSelf { replacement: self_ty }.visit_type_mut(&mut modified_self);
                    quote!(#modified_self)
                }
            }
        }).collect();
    
        let return_type = match &sig.output {
            syn::ReturnType::Type(_, ty) => Some(quote!(#ty)),
            syn::ReturnType::Default => None,
        };

        let entry = FuncEntry {
            name: func_name,
            path: path,
            args: arg_types,
            ret: return_type,
            has_selfarg,
        };

        database.push(entry);
    }

    database
}

// tokens as written in source, for error messages (quote's to_string separates all tokens by spaces)
fn display(tokens: TokenStream) -> String {
    let mut s = tokens.to_string();
    for (spaced, tight) in [(" :: ", "::"), (" < ", "<"), ("< ", "<"), (" >", ">"), (" ,", ","), ("& ", "&"), ("' ", "'")] {
        s = s.replace(spaced, tight);
    }
    s
}

// a reference type without 'static lifetime (arguments and receivers of spawnables must be 'static)
fn is_non_static_reference(ty: &Type) -> bool {
    match ty {
        Type::Reference(syn::TypeReference { lifetime, .. }) => !lifetime.as_ref().is_some_and(|lt| lt.ident == "static"),
        Type::Paren(paren) => is_non_static_reference(&paren.elem),
        _ => false,
    }
}

struct ReplaceSelf {
    replacement: Type,
}
impl VisitMut for ReplaceSelf {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if let Type::Path(type_path) = ty {
            if type_path.qself.is_none()
                && type_path.path.segments.len() == 1
                && type_path.path.segments[0].ident.to_string().to_lowercase() == "self" {
                *ty = self.replacement.clone();
                return;
            }
        }
        visit_mut::visit_type_mut(self, ty);
    }
}