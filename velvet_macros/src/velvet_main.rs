use std::{collections::HashSet, env};
use proc_macro::TokenStream;
use quote::quote;
use syn::{Expr, ItemFn, parse_quote, visit_mut::{self, VisitMut}};

/*
    adds velvet setup as the first thing in the function:
        - let mut root_worker = VelvetWorker::prepare_workers(num_workers, queue_size, steal);
        - root_worker.wait()
    and modifies the calls to spawnable functions to pass in the root-worker as an argument
*/
pub(super) fn build_velvet_main(targets: TokenStream, item: TokenStream) -> TokenStream {
    let mut ast = syn::parse_macro_input!(item as ItemFn);

    // MODIFY FUNCTION BODY TO CALL TARGET FUNCTIONS WITH ROOT_WORKER ARG
    modify_func_body(targets, &mut ast);

    // VELVET SETUP
    let queue_size = get_queue_size();
    // directly insert statements in the beginning of the function
    ast.block.stmts.splice(
        0..0,
        [
            parse_quote!(
                let num_workers = velvet_get_num_workers();
            ),
            parse_quote!(
                let mut __root__worker__ = velvet::VelvetWorker::prepare_workers(num_workers, #queue_size, crate::__velvet_steal__);
            ),
            parse_quote!(
                __root__worker__.wait();
            ),
        ],
    );

    quote!( #ast ).into()
}

/*
    adds the root_worker arg to all calls to target functions
    eg change 'spawnable(arg)' to 'spawnable(root_worker, arg)'
*/
fn modify_func_body(targets: TokenStream, ast: &mut ItemFn) {
    let root_worker: Expr = parse_quote! { &mut __root__worker__ };
    
    // collect the velvet-functions that must be re-written
    let target_functions: HashSet<String> = targets.to_string().split(',').map(|item| {
        item.trim().split('/').last().unwrap_or(item).to_string()
    }).collect();
    
    // visitor to re-write the calls to target functions
    let mut rewriter = ArgRewriter { targets: target_functions, arg_expr: &root_worker };
    rewriter.visit_item_fn_mut(ast);
}

/*
    visitor to iterate through the velvet-main function to find calls to the target functions
    all calls to target functions are rewritten to take the root_worker as an arg
 */
struct ArgRewriter <'ast>  {
    targets: HashSet<String>,
    arg_expr: &'ast Expr, // 'root_worker'
}
impl <'ast> VisitMut for ArgRewriter<'ast>{
    fn visit_expr_call_mut(&mut self, node: &mut syn::ExprCall) {
        if let Expr::Path(syn::ExprPath { path, .. }) = &*node.func {
            if let Some(seg) = path.segments.last() {
                if self.targets.contains(&seg.ident.to_string()) {
                    node.args.insert(0, self.arg_expr.clone());
                }
            }
        }

        visit_mut::visit_expr_call_mut(self, node);
    }

    fn visit_expr_method_call_mut(&mut self, node: &mut syn::ExprMethodCall) {
        if self.targets.contains(&node.method.to_string()) {
            node.args.insert(0, self.arg_expr.clone());
        }

        visit_mut::visit_expr_method_call_mut(self, node);
    }

}

// utility to get the work-queue size either from an envr variable or default
fn get_queue_size() -> usize {
    let queue_size = env::var("VELVET_QUEUE").ok();
    match queue_size {
        Some(string_value) => return string_value.parse::<usize>().expect("make sure VELVET_QUEUE env var is a positive integer"),
        None => return 64,
    }
}