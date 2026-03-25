use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TS2};
use quote::quote;
use syn::{Expr, ExprCall, ExprMethodCall, ExprPath, ItemFn, meta, parse::Result, parse_macro_input, parse_quote,
    Stmt, visit::{self, Visit}, visit_mut::{self, VisitMut}};

use super::{spawn_known::spawn_known, spawn_unknown::spawn_unknown, sync::sync};

pub(super) enum SyncInput {
    Known(usize, Option<Vec<Option<Stmt>>>), // vec[i] = statement corresponding to UID=i, with the func-call replaced with __SYNC_RES__
    Unknown(Option<Stmt>), // a single statement with the func-call replaced with __SYNC_RES__
}

pub(super) fn build_spawnable(attrs: TokenStream, item: TokenStream) -> TokenStream {
    let mut ast = parse_macro_input!(item as ItemFn);

    // parse arguments to macro
    let mut macro_args = SpawnableAttrs::default();
    let arg_parser = meta::parser(|nested_meta| macro_args.parse(nested_meta));
    parse_macro_input!(attrs with arg_parser);

    // add VelvetWorker as an argument to the function
    add_worker(&mut ast);

    // adds the 'spawn' logic
    let sync_input = if let Some(literal) = &macro_args.spawns {
        // case of known number of recursive calls
        let num_calls: usize = literal.base10_parse().unwrap();
        match spawn_known(&mut ast, num_calls) {
            Err(e) => return e.to_compile_error().into(),
            Ok(res) => res,
        }
    } else {
        // case of unknwon number of recursive calls
        match spawn_unknown(&mut ast) {
            Err(e) => return e.to_compile_error().into(),
            Ok(res) => res,
        }
    };

    // adds the 'sync' logic
     match sync(&mut ast, sync_input, &macro_args.shareds) {
        Err(e) => return e.to_compile_error().into(),
        Ok(()) => (),
    }

    quote!( #[allow(private_interfaces)] #ast ).into()
}

#[derive(Default)]
struct SpawnableAttrs {
    spawns: Option<syn::LitInt>,
    shareds: Vec<syn::LitStr>,
}
impl SpawnableAttrs {
    fn parse(&mut self, nested_meta: meta::ParseNestedMeta) -> Result<()> {
        if nested_meta.path.is_ident("spawns") {
            self.spawns = Some(nested_meta.value()?.parse()?);
            Ok(())
        } else if nested_meta.path.is_ident("shared") {
            let raw_list: syn::ExprArray = nested_meta.value()?.parse()?;
            self.shareds = raw_list.elems
                .into_iter()
                .map(|expr| match expr {
                    Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) => Ok(s),
                    _ => Err(syn::Error::new_spanned(expr, "expected string literal")),
                })
                .collect::<Result<_>>()?;
            Ok(())
        } else {
            Err(nested_meta.error("unsupported spawnable property"))
        }
    }
}

// adds __worker__ as first arg, taking care with &self params in methods
pub(super) fn add_worker(ast: &mut ItemFn) {
    let worker_arg: syn::FnArg = parse_quote! {
        __worker__: &mut velvet::VelvetWorker<crate::__Frame__>
    };
    // determine whether there is a self-param
    let idx = match ast.sig.inputs.first() {
        Some(syn::FnArg::Receiver(_)) => 1, // has `self`, insert after
        _ => 0, // plain function, insert at start
    };
    ast.sig.inputs.insert(idx, worker_arg);
}

// visitor that adds arg to a target function call
pub(super) struct AddArg <'ast> {
    pub(super) arg: Expr,
    pub(super) target: &'ast String,
}
impl <'ast> VisitMut for AddArg <'ast> {
    fn visit_expr_call_mut(&mut self, expr: &mut ExprCall) {
        // recurse first
        visit_mut::visit_expr_call_mut(self, expr);

        // extract the function-name part of the expression (path)
        if let Expr::Path(ExprPath { path, .. }) = &*expr.func {
             // check if it is a call to the target func
            if path.segments.last().map_or(false, |seg| seg.ident == &self.target) {
                expr.args.insert(0, self.arg.clone());
            }
        }
    }

    fn visit_expr_method_call_mut(&mut self, expr: &mut ExprMethodCall) {
        // recurse first
        visit_mut::visit_expr_method_call_mut(self, expr);

        // check if it is a call to the target method
        if expr.method == self.target {
            expr.args.insert(0, self.arg.clone());
        }
    }
}

// visitor to find target function calls and collects both their argument and full enclosing statement
pub(super) struct FindTarget<'stmt> {
    pub(super) target: &'stmt String,
    pub(super) args: Option<Vec<TS2>>,
    pub(super) ret_stmt: Option<Stmt>,
    pub(super) current_stmt: &'stmt Stmt,
}
impl <'stmt> Visit <'stmt> for FindTarget <'stmt>  {
    fn visit_stmt(&mut self, stmt: &'stmt Stmt) {
        self.current_stmt = stmt;
        visit::visit_stmt(self, stmt);
    }

    fn visit_expr_call(&mut self, expr: &'stmt ExprCall) {
        // recurse first
        visit::visit_expr_call(self, expr);

        // extract the function-name part of the expression (path)
        if let Expr::Path(ExprPath { path, .. }) = &*expr.func {
            // check if it is a call to the target func
            if path.segments.last().map_or(false, |seg| seg.ident.eq(self.target)) {
                // collect args, but remove any '&'
                // TODO: FINDING A '&' SHOULD THROW AN ERROR;  not threadsafe
                let quoted_args: Vec<_> = expr.args.iter().map(|arg|  {
                    let arg = match arg {
                        Expr::Reference(expr_ref) => &expr_ref.expr,
                        _ => arg,
                    };
                    quote!( #arg )
                }).collect();
                self.args = Some(quoted_args);
                self.ret_stmt = Some(self.current_stmt.clone());
            }
        }
    }

    fn visit_expr_method_call(&mut self, expr: &'stmt ExprMethodCall) {
        // recurse first
        visit::visit_expr_method_call(self, expr);

        // check if it is a call to the target method
        if expr.method.eq(self.target) {
            // the selfarg
            let receiver = &expr.receiver;
            let selfarg = quote!(#receiver);
            // collect args, but remove any '&'
            // TODO: FINDING A '&' SHOULD THROW AN ERROR;  not threadsafe
            let quoted_args: Vec<_> = std::iter::once(selfarg).chain(
                expr.args.iter().map(|arg| {
                    let arg = match arg {
                        Expr::Reference(expr_ref) => &expr_ref.expr,
                        _ => arg,
                    };
                    quote!( #arg )
            })).collect();
            self.args = Some(quoted_args);
            self.ret_stmt = Some(self.current_stmt.clone());
        }
    }
}

// function that returns a vector of indices of the argument in the provided signature that are references
pub(super) fn get_ref_indices(sig: &syn::Signature) -> Vec<usize> {
    let mut skipped_receiver = false;
    let mut ref_pos = Vec::new();
    for (idx, arg) in sig.inputs.iter().enumerate() {
        match arg {
            syn::FnArg::Typed(pat_type) => {
                if skipped_receiver && idx==1 || idx==0 { continue; } // skip worker-arg, which has already been added
                if let syn::Type::Reference(_) = &*pat_type.ty {
                    // this argument is a reference-type, meaning it will be converted to an Arc
                    // -1 because '&Worker' is already added, extra -1 if there was a receiver
                    let idx = if skipped_receiver { idx-2 } else { idx-1 };
                    ref_pos.push(idx);
                }
            },
            syn::FnArg::Receiver(_) => skipped_receiver = true,
        }
    }
    ref_pos
}

// replaces any call to target with replacement ident
pub(super) fn process_ret_stmt(name_str: &String, ret_stmt: Option<Stmt>) -> Option<Stmt> {
    struct Replacer<'a> {
        target: &'a str,
        replacement: syn::Ident,
    }
    impl<'a> VisitMut for Replacer<'a> {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            match expr {
                Expr::Call(call_expr) => {
                    if let Expr::Path(ref path_expr) = *call_expr.func {
                        if path_expr.path.segments.last().map(|s| s.ident == self.target).unwrap_or(false) {
                            // replace entire expression with the Ident
                            *expr = Expr::Path(ExprPath {
                                attrs: vec![],
                                qself: None,
                                path: self.replacement.clone().into(),
                            });
                            return;
                        }
                    }
                }
                Expr::MethodCall(method_call_expr) => {
                    if method_call_expr.method == self.target {
                        // replace entire expression with the Ident
                        *expr = Expr::Path(ExprPath {
                            attrs: vec![],
                            qself: None,
                            path: self.replacement.clone().into(),
                        });
                        return;
                    }
                }
                _ => {}
            }

            visit_mut::visit_expr_mut(self, expr);
        }
    }

    if let Some(stmt) = ret_stmt {
        let mut stmt = stmt.clone();
        let replacement = syn::Ident::new("__SYNC_RES__", Span::call_site());
        let mut replacer = Replacer {target: name_str, replacement };
        replacer.visit_stmt_mut(&mut stmt);

        let no_ret: Stmt = parse_quote!( __SYNC_RES__; );
        if no_ret == stmt { None } else { Some(stmt) }

    } else { None }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_ref_indices(){
        let ast: ItemFn = syn::parse_str(r#"
            fn foo(x: usize, y: &String, z: &bool, moo: i32, deng: &usize) -> usize {}
        "#).unwrap();
        let vec = get_ref_indices(&ast.sig);
        println!("FN: {:?}", vec);
    }

    #[test]
    fn test_get_ref_self_indices(){
        let ast: ItemFn = syn::parse_str(r#"
            fn foo(&self, x: usize, y: &String, z: &bool, moo: i32, deng: &usize) -> usize {}
        "#).unwrap();
        let vec = get_ref_indices(&ast.sig);
        println!("WITH SELF: {:?}", vec);
    }

    #[test]
    fn test_process_ret_stmt() {
        let name_str = String::from("foo");
        let stmt: Stmt = syn::parse_str(r#"acc = combined(acc, foo());"#).unwrap();
        let expected: Stmt = syn::parse_str(r#"acc = combined(acc, __SYNC_RES__);"#).unwrap();
        if let Some(stmt) = process_ret_stmt(&name_str, Some(stmt)) {
            println!("{:?}", stmt);
            assert_eq!(stmt, expected);
        } else {
            panic!("DIDNT GET PROCESSED STATEMENT !");
        }

        let stmt: Stmt = syn::parse_str(r#"foo();"#).unwrap();
        let stmt = process_ret_stmt(&name_str, Some(stmt));
        assert!(stmt.is_none());
    }
}