use proc_macro2::{Span, TokenStream as TS2};
use syn::{Block, Error, Expr, Ident, ItemFn, parse::Result, parse_quote, parse_str, Stmt, visit::Visit, visit_mut::VisitMut};
use super::spawnable::{AddArg, FindTarget, process_ret_stmt, SyncInput};
/*
    MODIFIES AST IN-PLACE
    given the number of recursive calls [n-many], do n-1 spawns and one 'local' recursive call last
    also adds spawned!(count) tags for sync-insertion. The count-argument is the index of the spawn,
    eg: spawned!(count) will match the count-th uid: variable name uid{COUNT}

    RETURNS:
    Option<Vec<Option<Stmt>>> which, if there are return values, is a vector of the statements using the variables
    vec[uid] = variable identifier for spawn with uid
*/
pub(super) fn spawn_known(ast: &mut ItemFn, num_calls: usize) -> Result<SyncInput> {
    let worker_expr: Expr = parse_quote!(__worker__);
    let name_ident = ast.sig.ident.clone();
    let name_str = name_ident.clone().to_string();
    let input_frame_str = format!("crate::__Frame__::Input{}", super::snake_to_pascal(&name_str));
    let input_frame_expr: Expr = syn::parse_str(&input_frame_str).unwrap();

    // variable to collect return-statements
    let ret_map = match ast.sig.output {
        syn::ReturnType::Default => None,
        _ =>  Some(Vec::new()),
    };

    // replace all but the last recursive call with spawns
    // in case of return values: return vector of return-variable statements
    let mut counted_replacer = CountReplace {
        counter: 0,
        count: num_calls-1,
        target: &name_str,
        frame_name: input_frame_expr,
        ret_map,
        error: None,
    };
    counted_replacer.visit_item_fn_mut(ast);
    if let Some(err) = counted_replacer.error {
        return Err(err);
    }

    // change remaining recursive calls to func(worker, ..);
    let mut rewriter = AddArg { arg: worker_expr, target: &name_str };
    rewriter.visit_item_fn_mut(ast);

    Ok(SyncInput::Known(num_calls, counted_replacer.ret_map))
}

struct CountReplace<'ast> {
    counter: usize,
    count: usize,
    target: &'ast String,
    frame_name: Expr,
    ret_map: Option<Vec<Option<Stmt>>>, // index in vector = UID-idx; value = Stmt with func-call
    error: Option<Error>,
}
impl <'ast> VisitMut for CountReplace<'ast> {
    /*
        since a spawn is actually two statements replacing one, they cannot be inserted at the statement-level,
        but have to be inserted at the block-level.

        strategy: inspect each statement in a block as to whether it contains a call to the target,
        collect the statements in the block, but if the target is found, generate the spawn-statements
        then, replace the entire block with the collected statements
    */
    fn visit_block_mut(&mut self, block: &mut Block){
        let mut new_stmts = Vec::new();

        for mut stmt in block.stmts.drain(..) {
            self.visit_stmt_mut(&mut stmt);

            if self.counter < self.count {
                // check statement if it contains a call to the target
                // if it does, the arguments to the method are returned
                if let Some(args) = self.contains_target(&stmt) {
                    // create spawn statements:
                    // let uid_#COUNT = __worker__.get_seq();
                    // __worker__.spawn(crate::__Frame__::InputFrame(uid_#COUNT, args..));'
                    let uid = Ident::new(&format!("__{}__", self.counter), Span::call_site());
                    let uid_stmt: Stmt = parse_quote!(let #uid = __worker__.get_seq(););
                    let frame_name = &self.frame_name;
                    let spawn_stmt: Stmt = parse_quote!(__worker__.spawn(#frame_name(#uid, #(#args),*)););
                    new_stmts.push(uid_stmt);
                    new_stmts.push(spawn_stmt);
                    #[cfg(feature = "stats")]
                    new_stmts.push(parse_quote! {__worker__.add_spawns(1);});
                    // add 'spawned' tag for sync-phase
                    let spawned_str = format!("spawned!({});", self.counter);
                    new_stmts.push(parse_str(&spawned_str).unwrap());
                    self.counter += 1;
                    continue;
                }
            }
            // we are done replacing statements
            new_stmts.push(stmt);
        }

        // replace block-statements with the new ones
        block.stmts = new_stmts;
    }
}
impl <'ast> CountReplace<'ast> {
    /*
        checks whether the statement includes a call to the target
        Returns an Option holding the quoted arguments to the func, in case it was found
    */
    fn contains_target(&mut self, stmt: &Stmt) -> Option<Vec<TS2>> {
        let mut visitor = FindTarget{ target: self.target, args: None, ret_stmt: None, current_stmt: stmt };
        visitor.visit_stmt(stmt);

        if  visitor.args.is_some() {
            // target was found; collect the ret-stmt if necessary
            if let Some(ref mut ret_map) = self.ret_map {
                let stmt = process_ret_stmt(self.target, visitor.ret_stmt);
                ret_map.push(stmt);
            }
        }

        visitor.args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn testing(){
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo(&self, i: usize, x: &i32) {
                self.foo(i, x);
                self.foo(i, x);
            }
        "#).unwrap();

        super::super::spawnable::add_worker(&mut ast);

        let num_calls = 2;
        let name_ident = ast.sig.ident.clone();
        let name_str = name_ident.clone().to_string();
        let input_frame_str = format!("crate::__Frame__::Input{}", super::super::snake_to_pascal(&name_str));
        let input_frame_expr: Expr = syn::parse_str(&input_frame_str).unwrap();

        let mut counted_replacer = CountReplace {
            counter: 0,
            count: num_calls-1,
            target: &name_str,
            frame_name: input_frame_expr,
            ret_map: None,
            error: None,
        };

        counted_replacer.visit_item_fn_mut(&mut ast);


        println!("COUNTER: {} \t COUNT: {} \t TARGET: {} \t FRAME_NAME: {:?} ",
                    counted_replacer.counter,counted_replacer.count, counted_replacer.target, counted_replacer.frame_name);
    }
}