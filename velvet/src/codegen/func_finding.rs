use std::{collections::HashMap, error::Error, fs, path::{Path as path, PathBuf}, process::exit};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{self, FnArg, Ident, Path, Signature, Type, UseTree, visit_mut::{self, VisitMut}};

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
}
impl VisitMut for FnVisitor {
    fn visit_item_mod_mut(&mut self, node: &mut syn::ItemMod) {
        // need to push this nested module name onto the path
        let mod_name = node.ident.to_string();
        let old_path = self.qualified_path.clone();
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
                if is_spawnable(&method.attrs) {
                    // re-write method signature with qualified types
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
        if is_spawnable(&node.attrs) {
            // re-write function signature with qualified types
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

        // if type is imported, re-write it to its fully qualified path
        if let Some(top_level) = node.path.segments.first() {
            let ident = &top_level.ident;
            let name = ident.to_string();
            if is_primitive(&name) {
                return; // do not rewrite primitives
            }

            if let Some(full_path_str) = self.import_map.get(&name) {
                // save arguments to transfer to the full-path version
                let args = &top_level.arguments;

                // parse the full path string into a Path
                let mut full_path: Path = syn::parse_str(full_path_str).expect(&format!("could not parse {}",full_path_str));
                
                // re-attach arguments from 'top-level' to the fully qualified path
                if let Some(last_seg) = full_path.segments.last_mut() {
                    last_seg.arguments = args.clone();
                }

                // overwrite
                node.path = full_path;
                return;
            }

            println!("cargo::Error=Could not detect qualified path for non-primitive type : {}. Make sure it is explicitly imported", name);
        }
    }
}
impl FnVisitor {
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
                self.import_map.insert(ident.to_string(), full_path);
            }
            UseTree::Rename(syn::UseRename { ident, rename, .. }) => {
                let full_path = if prefix.is_empty() {
                    ident.to_string()
                } else {
                    format!("{prefix}::{}", ident)
                };
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
        "f32" | "f64" | "String" | "Vec")
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
                println!("cargo::Error=Error: while looking for spawnable functions in file {:#?} ; {}", filepath, e);
                exit(1);
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
    };
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

        let arg_types: Vec<_> = sig.inputs.iter().enumerate().map(|(idx, arg)| {
            match arg {
                FnArg::Typed(pat_type) => {
                    let ty = &*pat_type.ty;

                    if let Type::Reference(syn::TypeReference { .. }) = ty {
                        let msg = format!("Reference arguments are not supported for Spawnable functions. Reference found in function {} at arg position {}", func_name, idx);
                        println!("cargo:warning={}", msg);
                        std::process::exit(1);
                    }
                    quote!(#ty)
                },
                FnArg::Receiver(recv) => {
                    let selftype =  selftype.as_ref().unwrap();
                    let self_ty = syn::parse_str::<Type>(selftype).expect(&format!("Could not parse {} into a type", selftype));
                    if recv.colon_token.is_some() {
                        // want to full qualified type, but with actual selftype instead of 'self'
                        let mut modified_self = *recv.ty.clone();
                        ReplaceSelf { replacement: self_ty }.visit_type_mut(&mut modified_self);
                        quote!(#modified_self)
                    } else if recv.reference.is_some() {
                        let msg = format!("Reference receivers ('&self') are not supported for Spawnable methods. Use explicit types such as Box<Self> or Arc<Self>. \n Reference receiver found in methid {}", func_name);
                        println!("cargo:warning={}", msg);
                        std::process::exit(1);
                    } else {
                        quote!(#self_ty)
                    }
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