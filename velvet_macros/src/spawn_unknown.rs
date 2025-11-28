use proc_macro2::TokenStream as TS2;
use syn::{Block, Expr, ExprCall, ExprMethodCall, ExprPath, parse::Result, parse_quote, Stmt, 
    visit::{self, Visit}, visit_mut::{self, VisitMut}};
use super::spawnable::{AddArg, FindTarget, get_ref_indices, process_ret_stmt, SyncInput};

/*
    IDENTIFY CONTEXT: non-loop block(s) || one, non-nested for-loop with max 6 recursive calls || rest [loops, nested/multiple]

    if non-loop block(s)
        ASSUME PROGRAMMER JUST DIDNT PROVIDE spawns ARG
        -> call super::spawn_known(nr-of-calls)
        ** if spawning in multiple blocks, the recurse-direct optimisation might not actually be executed. but OK.

    if single non-nested for-loop with detectable nr of iters
        - compute nr of recursive calls. if < 7, call function to unroll loop and apply direct-recursion optimisation

    else
        - add boilerplate at top: seq-checkpoint and count
        - re-write *ALL* recursive calls to spawns

    RETURNS: SyncInput
    -> in non-loop cases: tuple with nr of spawns (= nr of uids) and a vec of statements w/spawnable call replaced with __SYNC_RES__
    -> in loop cases: single statement where spawnable is called from, with actual call replaced with __SYNC_RES__ (or None if void)
*/
pub(super) fn spawn_unknown(ast: &mut syn::ItemFn) -> Result<SyncInput> {
    let name_str = ast.sig.ident.clone().to_string();

    // detect context
    let mut context_visitor = ContextDetector {
        target: &name_str,
        loop_depth: 0,
        for_loop_iters: None,
        for_loops_with_calls: 0,
        non_loop_calls: 0,
        nested_call: false,
        blocks: HashSet::new(),
        current_block: None,
        total_calls: 0,
    };
    context_visitor.visit_item_fn(&ast);

    let total_calls = context_visitor.total_calls;

    if total_calls == 0 {
        let span = proc_macro2::Span::call_site();
        let msg = format!("No recursive calls found in spawnable-tagged function {}", name_str);
        return Err(syn::Error::new(span,msg));
    }

    if context_visitor.non_loop_calls == total_calls {
        // non-loop block(s); can use spawn_known
        super::spawn_known::spawn_known(ast, total_calls)
    } else {
        // there are recursive calls in loops
        if context_visitor.for_loop_iters.is_some() && context_visitor.blocks.len() == 1 {
            // single, non-nested for-loop with extractable nr of iterations
            let iters = context_visitor.for_loop_iters.unwrap();
            if iters * context_visitor.total_calls < 7 {
                return single_loop_context(ast, &name_str);
            }
        }
        spawn_all_context(ast, &name_str)
    }
}

/*
    - add boilerplate at top: seq-checkpoint and count
    - detect/find loop
    - duplicate block inside loop
    - reduce loop range by 1
    - first block: spawn logic for all recursive calls
    - second block: assign iter-var to last loop-range value, add worker-arg to (last) recursive call
    - return Option<Stmt> with return-val-assignment64
*/
fn single_loop_context(ast: &mut syn::ItemFn, name_str: &String) -> Result<SyncInput> {
    // insert seq_checkpoint and counter
    let checkpoint_stmt: Stmt = parse_quote!(let mut __checkpoint__ = __worker__.get_seq(););
    let count_stmt: Stmt = parse_quote!(let mut __count__ = 0;);
    ast.block.stmts.splice(0..0, [checkpoint_stmt, count_stmt]);

    // rewrite the for-loop with recursive calls [NOTE: undefined behavour for nested loops, and if there are recursive calls in multiple loops]
    let mut loop_visitor = LoopRewriter { target: name_str, ref_idx: get_ref_indices(&ast.sig), done: false, ret_stmt: None };
    loop_visitor.visit_item_fn_mut(ast);

    let ret_stmt = match ast.sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(..) => process_ret_stmt(name_str, loop_visitor.ret_stmt),
    };

    Ok(SyncInput::Unknown(ret_stmt))
}

/*
    - add boilerplate at top: seq-checkpoint and count
    - re-write *ALL* recursive calls to spawns
*/
fn spawn_all_context(ast: &mut syn::ItemFn, name_str: &String) -> Result<SyncInput> {
    // insert seq_checkpoint and counter
    let checkpoint_stmt: Stmt = parse_quote!(let mut __checkpoint__ = __worker__.get_seq(););
    let count_stmt: Stmt = parse_quote!(let mut __count__ = 0;);
    ast.block.stmts.splice(0..0, [checkpoint_stmt, count_stmt]);

    // rewrite spawns
    let input_frame_str = format!("crate::__Frame__::Input{}", super::snake_to_pascal(name_str));
    let input_frame_expr: Expr = syn::parse_str(&input_frame_str).unwrap();
    let ref_idx = get_ref_indices(&ast.sig);
    let mut spawn_replacer = SpawnReplace {
        count: None,
        insert_spawned: true,
        target: name_str,
        ref_idx: &ref_idx,
        frame_name: input_frame_expr,
        ret_stmt: None,
    };
    spawn_replacer.visit_item_fn_mut(ast);

    let ret_stmt = match ast.sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(..) => process_ret_stmt(name_str, spawn_replacer.ret_stmt),
    };

    Ok(SyncInput::Unknown(ret_stmt))
}


struct LoopRewriter<'ast> {
    target: &'ast String,
    ref_idx: Vec<usize>,
    done: bool,
    ret_stmt: Option<Stmt>,
}
impl <'ast> VisitMut for LoopRewriter<'ast> {
    /*
        - detect/find loop [ASSUMPTION: NO NESTED LOOPS]
        - replace *ENTIRE LOOP* with:
            - TWO BLOCKS:
                - first loop-block copy: 
                    - loop bound is reduced by 1
                    - spawn logic for all recursive calls
                - second loop-block copy: 
                    - replace loop stmt with assignment of iter-var to last loop-range value
                    - spawn logic for all except the last recursive call
                    - add worker-arg to last recursive call
            - OR, in case of inability to rewrite loop bound: all recursive calls become spawns
        - set ret_stmt Option<Stmt> with return-val-assignment
    */
    fn visit_block_mut(&mut self, block: &mut Block) {
        // will be replacing the statements in this block
        let mut new_stmts = Vec::new();

        for stmt in block.stmts.iter_mut() {
            // if we have already found and replaced the block, we are done
            if self.done {
                new_stmts.push(stmt.clone());
                continue;
            }

            let mut found = false;

            // check and transform stmt if it is a loop
            self.process_stmt(stmt, &mut new_stmts, &mut found);

            // clone if stmt is not a loop/modified
            if !found { new_stmts.push(stmt.clone()); }
        }

        // replace block
        block.stmts = new_stmts;
    }
}
impl <'ast>  LoopRewriter <'ast> {
    fn process_stmt(&mut self, stmt: &mut Stmt, new_stmts: &mut Vec<Stmt>, found: &mut bool) {
        // check loops!
        // we are only re-writing For-loops!
        if let Stmt::Expr(expr, _) = stmt {
            match expr {
                Expr::ForLoop(expr_for) => {
                    // check if loop body contains the target call
                    let mut finder = CallFinder { target: &self.target, found: false };
                    finder.visit_block(&expr_for.body);
                    
                    if finder.found {                               
                        // generate the new code
                        let mut generated = self.generate(expr);
                        new_stmts.append(&mut generated);
                        *found = true;
                        self.done = true;
                        return;
                    }
                },
                // directly skip other loops
                Expr::While(_) => (),
                Expr::Loop(_) => (),
                // recurse
                _ => visit_mut::visit_expr_mut(self, expr),
            }
        }
    }

    /*  
        ASSUME: For-loop, literals in range.

        GOAL:
        for ... [loop-bound reduced by 1] {    
            [ BODY WITH SPAWN LOGIC...]
        }

        {
            [ BODY WITH WORKER IN RECURSIVE CALL ARG ...]
        }
    */
    fn generate(&mut self, expr: &Expr) -> Vec<Stmt> {
        // attempt to reduce range on for-loop and make a statement that assigns the max val to the looping variable, if it exists
        if let Some((mut for_loop, assignment_stmt)) = self.rewrite_for(expr.clone()) {
            
            // generate two different versions of the body (spawn & recurse)
            let mut rec_body = for_loop.body.clone();
            let ret_stmt1 = self.gen_spawn(&mut for_loop.body);
            let ret_stmt2 = self.gen_rec(&mut rec_body);

            let body: Block = if let Some(stmt) = assignment_stmt {
                parse_quote! {
                    {
                        #for_loop
                        #stmt
                        #rec_body
                        spawned!();
                    }
                } 
            } else {
                parse_quote! {
                    {
                        #for_loop
                        #rec_body
                        spawned!();
                    }
                }
            };

            self.ret_stmt = if ret_stmt1.is_none() { ret_stmt2 } else { ret_stmt1 };

            body.stmts
        } else {
            // could not extract the iterable logic, so just spawn everything
            let mut spawn_all = expr.clone();
            self.ret_stmt = self.expr_spawn(&mut spawn_all);
            let body: Block = parse_quote! {
                {
                    #spawn_all
                    spawned!();
                }
            };
            body.stmts
        }
    }

    // replace all recursive calls with spawns
    fn gen_spawn(&self, block: &mut Block) -> Option<Stmt> {
        let input_frame_str = format!("crate::__Frame__::Input{}", super::snake_to_pascal(&self.target));
        let input_frame_expr: Expr = syn::parse_str(&input_frame_str).unwrap();

        let mut spawn_replacer = SpawnReplace {
            count: None,
            insert_spawned: false,
            target: &self.target,
            ref_idx: &self.ref_idx,
            frame_name: input_frame_expr,
            ret_stmt: None,
        };
        spawn_replacer.visit_block_mut(block);

        spawn_replacer.ret_stmt
    }

    // last loop body;
    // - count nr of recursive calls
    // - insert spawns for count-1 calls
    // - add 'worker' to last recursive call
    fn gen_rec(&self, block: &mut Block) -> Option<Stmt> {
        let input_frame_str = format!("crate::__Frame__::Input{}", super::snake_to_pascal(&self.target));
        let input_frame_expr: Expr = syn::parse_str(&input_frame_str).unwrap();

        struct CallCounter <'block> { count: usize, target: &'block String }
        impl<'block> Visit <'block> for CallCounter <'block> {
            fn visit_expr_method_call(&mut self, expr: &'block ExprMethodCall) {
                if expr.method == self.target {
                    self.count += 1;
                }
                visit::visit_expr_method_call(self, expr);
            }

            fn visit_expr_call(&mut self, expr: &'block ExprCall) {
                if let Expr::Path(ExprPath { path, .. }) = &*expr.func {
                    if path.segments.last().map_or(false, |seg| seg.ident == &self.target) {
                        self.count += 1;
                    }
                }
                visit::visit_expr_call(self, expr);
            }
        }
        let mut counter = CallCounter { count: 0, target: &self.target };
        counter.visit_block(&block);

        // replace (count-1)-many spawns
        let ret_stmt = if counter.count > 1 {
            let mut spawn_replacer = SpawnReplace {
                count: Some((counter.count, 1)),
                insert_spawned: false,
                target: &self.target,
                ref_idx: &self.ref_idx,
                frame_name: input_frame_expr,
                ret_stmt: None,
            };
            spawn_replacer.visit_block_mut(block);
            spawn_replacer.ret_stmt
        } else { None };

        // add worker-arg to remaining recursive call
        let worker_expr: Expr = parse_quote!(__worker__);
        let mut arg_adder = AddArg { arg: worker_expr, target: &self.target };
        arg_adder.visit_block_mut(block);

        ret_stmt
    }

    fn rewrite_for(&self, expr: Expr) -> Option<(syn::ExprForLoop, Option<Stmt>)> {
        if let Expr::ForLoop(mut expr_for) = expr {
            // extract range
            if let Some((start, end, incl)) = check_for_loop_bounds(&expr_for.expr) {
                let start_usize = start as usize;
                let new_end = (end - 1) as usize;
                let new_range: Expr = if incl == 1 {
                    parse_quote! { #start_usize ..= #new_end }
                } else {
                    parse_quote! { #start_usize .. #new_end }
                };
                expr_for.expr = Box::new(new_range);

                // if pattern introduces a variable, bind it to original upper bound value
                let final_iter = if incl == 1 { end as usize } else { new_end };
                let binding = if let syn::Pat::Ident(pat_ident) = &*expr_for.pat {
                    let ident = pat_ident.ident.clone();
                    let binding: Stmt = parse_quote! { let #ident = #final_iter; };
                    Some(binding)
                } else { None };
                
                return Some((expr_for, binding));
            }
        }
        return None;
    }

    fn expr_spawn(&self, expr: &mut Expr) -> Option<Stmt>{
        match expr {
            Expr::ForLoop(expr_for) => {
                let mut spawned = expr_for.body.clone();
                let ret_stmt = self.gen_spawn(&mut spawned);
                expr_for.body = spawned;
                ret_stmt
            },
            Expr::While(expr_while) => {
                let mut spawned = expr_while.body.clone();
                let ret_stmt = self.gen_spawn(&mut spawned);
                expr_while.body = spawned;
                ret_stmt
            }
            Expr::Loop(expr_loop) => { 
                let mut spawned = expr_loop.body.clone();
                let ret_stmt = self.gen_spawn(&mut spawned);
                expr_loop.body = spawned;
                ret_stmt
            }
            _ => None // should be unreachable because this is only called on loops
        }
    }
}

struct CallFinder <'ast> {
    target: &'ast String,
    found: bool,
}
impl <'ast> Visit<'ast> for CallFinder<'ast> {
    // check if it is a call to the target method
    fn visit_expr_method_call(&mut self, expr: &'ast ExprMethodCall) {
        if self.found { return; }
        
        if expr.method == self.target {
            self.found = true;
            return;
        }
        
        // recurse if not found
        visit::visit_expr_method_call(self, expr);

    }

    fn visit_expr_call(&mut self, expr: &'ast ExprCall) {
        if self.found { return; }
        
        // extract the function-name part of the expression (path)
        if let Expr::Path(ExprPath { path, .. }) = &*expr.func {
            // check if it is a call to the target func
            if path.segments.last().map_or(false, |seg| seg.ident == &self.target) {
                self.found = true;
                return;
            }
        }

        // recurse if not found
        visit::visit_expr_call(self, expr);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if self.found { return; } // return if we already found target
        visit::visit_expr(self, expr); // else recurse
    }
}

struct SpawnReplace<'block> {
    count: Option<(usize, usize)>, // in case we are doing a 'counted replace'
    insert_spawned: bool, // whether or not to insert the spawned-tag
    target: &'block String,
    frame_name: Expr,
    ref_idx: &'block Vec<usize>,
    ret_stmt: Option<Stmt>, // assignment stmt in case there is a return value. ASSUMES IT IS ALWAYS THE SAME
}
impl <'block> VisitMut for SpawnReplace<'block> {
    /*
        since a spawn is actually multiple statements (spawn + count-increment) replacing one (recursive call),
        they cannot be inserted at the statement-level, but have to be inserted at the block-level.

        strategy: inspect each statement in a block as to whether it contains a call to the target,
        collect the statements in the block, but if the target is found, generate the spawn-statements
        then, replace the entire block with the collected statements
    */
    fn visit_block_mut(&mut self, block: &mut Block){
        let mut new_stmts = Vec::new();

        for mut stmt in block.stmts.drain(..) {
            self.visit_stmt_mut(&mut stmt);

            // check statement if it contains a call to the target
            // if it does, the arguments to the method are returned
            if let Some(args) = self.contains_target(&stmt) {
                // if we are doing a 'counted' replace, check and increment count
                if let Some((target, curr)) = self.count {
                    if curr < target {
                        // increment curr
                        self.count = Some((target, curr+1));
                    } else {
                        // we are done; keep as-is, worker-arg will be added later
                        new_stmts.push(stmt);
                        continue;
                    }
                }

                // create spawn statements:
                // let __uid__ = __worker__.get_seq();
                // __worker__.spawn(crate::__Frame__::InputFrame(__uid__, args..));'
                let frame_name = &self.frame_name;
                let uid_stmt: Stmt = parse_quote!(let __uid__ = __worker__.get_seq(););
                let spawn_stmt: Stmt = parse_quote!(__worker__.spawn(#frame_name(__uid__, #(#args),*)););
                // increment counter!
                let count_stmt: Stmt = parse_quote!(__count__ += 1;);
                
                new_stmts.push(uid_stmt);
                new_stmts.push(spawn_stmt);
                new_stmts.push(count_stmt);
                #[cfg(feature = "stats")]
                new_stmts.push(parse_quote! {__worker__.add_spawns(1);});

                if self.insert_spawned { new_stmts.push(parse_quote!(spawned!();)); }

                continue;
            }
            // if we made it this far, the stmt does not contain a recursive call; keep as-is
            new_stmts.push(stmt);
        }

        // replace block-statements with the new ones
        block.stmts = new_stmts;
    }
}
impl <'block> SpawnReplace <'block> {
    /*
        checks whether the statement includes a call to the target
        Returns an Option holding the quoted arguments to the func, in case it was found
    */
    fn contains_target(&mut self, stmt: &Stmt) -> Option<Vec<TS2>> {
        let mut visitor = FindTarget{ target: &self.target, args: None, ref_idx: &self.ref_idx, ret_stmt: None, current_stmt: stmt };
        visitor.visit_stmt(stmt);

        if let Some(stmt) = visitor.ret_stmt {
            if self.ret_stmt.is_none() {
                self.ret_stmt = Some(stmt);
            }
        }
        visitor.args
    }
}

use std::collections::HashSet;
struct ContextDetector <'ast> {
    target: &'ast String,
    loop_depth: usize, // current loop depth
    for_loops_with_calls: usize, // 
    for_loop_iters: Option<usize>, // number of iters top-level for-loop, if detectable
    non_loop_calls: usize, // calls outside of loops (in case there is both loop and non-loop recursion)
    nested_call: bool, // just need to know if there are nested loops

    blocks: HashSet<*const Block>, // cardinality = number of distinct blocks with target calls
    current_block: Option<*const Block>, // for tracking where calls appear

    total_calls: usize,
}
impl<'a> ContextDetector<'a> {
    fn record_call(&mut self) {
        self.total_calls += 1;
        if let Some(block_ptr) = self.current_block {
            self.blocks.insert(block_ptr);
        }
        if self.loop_depth == 0 {
            self.non_loop_calls += 1;
        } else if self.loop_depth > 1 {
            self.nested_call = true;
        }
    }
}
impl <'ast> Visit<'ast> for ContextDetector<'ast> {
    fn visit_block(&mut self, block: &'ast Block) {
        // track current block pointer to know where calls appear
        let old_block = self.current_block;
        self.current_block = Some(block as *const _);
        visit::visit_block(self, block);
        self.current_block = old_block;
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        match expr {
            Expr::ForLoop(forloop) => {
                // check whether any recursive calls are made inside this for-loop
                let before_calls = self.total_calls;
                self.loop_depth += 1;
                visit::visit_expr(self, expr);
                self.loop_depth -= 1;
                if (self.total_calls - before_calls) > 0 {
                    self.for_loops_with_calls += 1;
                    if self.for_loops_with_calls == 1  && self.loop_depth == 0  && before_calls == 0 && !self.nested_call {
                        // try to decipher nr of iters 
                        // (only bother if it is the only for-loop with calls, it is non-nested and we had not any calls so far)
                        self.for_loop_iters = if let Some((start, end, inclusive)) = check_for_loop_bounds(&forloop.expr) {
                            if end >= start { Some((end - start + inclusive) as usize) }
                            else { Some(0) } 
                        } else { None };
                    }
                }
            }
            Expr::While(_) | Expr::Loop(_) => {
                self.loop_depth += 1;
                visit::visit_expr(self, expr);
                self.loop_depth -= 1;
            }
            Expr::Call(ExprCall { func, .. }) => {
                if let Expr::Path(ExprPath { path, .. }) = &**func {
                    if path.segments.last().map_or(false, |seg| seg.ident == &self.target) {
                        self.record_call();
                    }
                }
                visit::visit_expr(self, expr);
            }
            Expr::MethodCall(ExprMethodCall { method, .. }) => {
                if method == self.target {
                    self.record_call();
                }
                visit::visit_expr(self, expr);
            }
            _ => {
                visit::visit_expr(self, expr);
            }
        }
    }
}

fn check_for_loop_bounds(mut expr: &Expr) -> Option<(isize, isize, isize)> {
        // strip parentheses
        while let Expr::Paren(paren) = expr { expr = &paren.expr; }

        if let Expr::Range(syn::ExprRange { start, end, limits, .. }) = expr {
            // try to extract bounds
            if let (Some(start), Some(end)) = (start.as_deref(), end.as_deref()) {
                // try to extract bound values
                if let (Some(start_val), Some(end_val)) = (extract_literal(start), extract_literal(end)) {
                    // calculate nr of iters
                    let inclusive = match limits {
                        syn::RangeLimits::HalfOpen(_) => 0,
                        syn::RangeLimits::Closed(_) => 1,
                    };
                    return Some((start_val, end_val, inclusive))
                }
            }
        }
        return None;
}

// extract integer literal from an expression if possible
fn extract_literal(mut expr: &Expr) -> Option<isize> {
    // strip parentheses
    while let Expr::Paren(paren) = expr { expr = &paren.expr; }

    match expr {
        Expr::Lit(lit) => {
            if let syn::Lit::Int(int_lit) = &lit.lit {
                int_lit.base10_parse().ok()
            } else {
                None
            }
        }

        // unary minus (-5)
        Expr::Unary(u) => {
            if let syn::UnOp::Neg(_) = u.op {
                let val = extract_literal(&u.expr)?;
                let neg = 0 - val;
                Some(neg)
            } else {
                None
            }
        }

        // casts (5 as usize)
        Expr::Cast(c) => extract_literal(&c.expr),

        _ => None,
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_context_nested1() {
        let ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..10 {
                    some_fn();
                    if true {
                        foo();
                        foo();
                    }
                }
            }
        "#).unwrap();
        let foo = String::from("foo");
        let mut visitor = ContextDetector {
            target: &foo,
            loop_depth: 0,
            for_loops_with_calls: 0,
            for_loop_iters: None,
            non_loop_calls: 0,
            nested_call: false,
            blocks: HashSet::new(),
            current_block: None,
            total_calls: 0,
        };
        
        visitor.visit_item_fn(&ast);
        println!("Analysis: {}", analyse(&visitor));
    }

    #[test]
    fn test_detect_context_nested2() {
        let ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                if true {
                    for i in -1..50 {
                        some_fn();
                        foo();
                        foo();
                    }
                }
            }
        "#).unwrap();
        let foo = String::from("foo");
        let mut visitor = ContextDetector {
            target: &foo,
            loop_depth: 0,
            for_loops_with_calls: 0,
            for_loop_iters: None,
            non_loop_calls: 0,
            nested_call: false,
            blocks: HashSet::new(),
            current_block: None,
            total_calls: 0,
        };
        
        visitor.visit_item_fn(&ast);
        println!("Analysis: {}", analyse(&visitor));
    }

    #[test]
    fn test_detect_context_nested_loop() {
        let ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..5 {
                    some_fn();
                    while true {
                        foo();
                        foo();
                    }
                }
                if true {
                    println!("hello");
                }
            }
        "#).unwrap();
        let foo = String::from("foo");
        let mut visitor = ContextDetector {
            target: &foo,
            loop_depth: 0,
            for_loops_with_calls: 0,
            for_loop_iters: None,
            non_loop_calls: 0,
            nested_call: false,
            blocks: HashSet::new(),
            current_block: None,
            total_calls: 0,
        };
        
        visitor.visit_item_fn(&ast);
        println!("Analysis: {}", analyse(&visitor));
    }

    #[test]
    fn test_detect_context_top_level() {
        let ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in [1,2,3] {
                    some_fn();
                    foo();
                    foo();
                    another_fn();
                    foo();
                }
            }
        "#).unwrap();
        let foo = String::from("foo");
        let mut visitor = ContextDetector {
            target: &foo,
            loop_depth: 0,
            for_loops_with_calls: 0,
            for_loop_iters: None,
            non_loop_calls: 0,
            nested_call: false,
            blocks: HashSet::new(),
            current_block: None,
            total_calls: 0,
        };
        
        visitor.visit_item_fn(&ast);
        println!("Analysis: {}", analyse(&visitor));
    }

    #[test]
    fn test_detect_context_no_target() {
        let ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..5 {
                    some_fn();
                }
                if true {
                    println!("hello");
                }
            }
        "#).unwrap();
        let foo = String::from("foo");
        let mut visitor = ContextDetector {
            target: &foo,
            loop_depth: 0,
            for_loops_with_calls: 0,
            for_loop_iters: None,
            non_loop_calls: 0,
            nested_call: false,
            blocks: HashSet::new(),
            current_block: None,
            total_calls: 0,
        };
        
        visitor.visit_item_fn(&ast);
        println!("Analysis: {}", analyse(&visitor));
    }

    #[test]
    fn test_detect_context_two_fors() {
        let ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..5 {
                    some_fn();
                    foo();
                    foo();
                }
                for i in 0..10 {
                    some_fn();
                    foo();
                    foo();
                }
            }
        "#).unwrap();
        let foo = String::from("foo");
        let mut visitor = ContextDetector {
            target: &foo,
            loop_depth: 0,
            for_loops_with_calls: 0,
            for_loop_iters: None,
            non_loop_calls: 0,
            nested_call: false,
            blocks: HashSet::new(),
            current_block: None,
            total_calls: 0,
        };
        
        visitor.visit_item_fn(&ast);
        println!("Analysis: {}", analyse(&visitor));
    }

    #[test]
    fn test_detect_context_mixed() {
        let ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..5 {
                    some_fn();
                    foo();
                    foo();
                }
                foo();
                foo();
                while(true) {
                    foo();
                }
            }
        "#).unwrap();
        let foo = String::from("foo");
        let mut visitor = ContextDetector {
            target: &foo,
            loop_depth: 0,
            for_loops_with_calls: 0,
            for_loop_iters: None,
            non_loop_calls: 0,
            nested_call: false,
            blocks: HashSet::new(),
            current_block: None,
            total_calls: 0,
        };
        
        visitor.visit_item_fn(&ast);
        println!("Analysis: {}", analyse(&visitor));
    }

    fn analyse(visitor: &ContextDetector) -> String {
        format!("\n
        for_loops_with_calls: {} \n 
        for_loop_iters: {:?} \n
        non_loop_calls: {} \n
        total_calls: {} \n
        nested_call: {} \n
        blocks: {}",
        visitor.for_loops_with_calls, visitor.for_loop_iters, visitor.non_loop_calls, visitor.total_calls, visitor.nested_call, visitor.blocks.len())
    }

    #[test]
    fn test_ret_expr_loop() {

        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                let vect = Vec::new();
                for i in 0..5 {
                    foo(1);
                    foo(2);
                }
            }
        "#).unwrap();
        let expected: Stmt = syn::parse_str(r#"foo(1);"#).unwrap();
        
        let foo = String::from("foo");
        let mut loop_visitor = LoopRewriter { target: &foo, done: false, ref_idx: get_ref_indices(&ast.sig), ret_stmt: None };
        loop_visitor.visit_item_fn_mut(&mut ast);

        if let Some(stmt) = loop_visitor.ret_stmt {
            assert_eq!(stmt, expected);
        } else {
            panic!("DIDNT GET STATEMENT 1!");
        }

        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                let vect = Vec::new();
                for i in 0..5 {
                    vect.push(foo());
                    vect.push(foo());
                }
            }
        "#).unwrap();
        let expected: Stmt = syn::parse_str(r#"vect.push(foo());"#).unwrap();
        
        let foo = String::from("foo");
        let mut loop_visitor = LoopRewriter { target: &foo, done: false, ref_idx: get_ref_indices(&ast.sig), ret_stmt: None };
        loop_visitor.visit_item_fn_mut(&mut ast);

        if let Some(stmt) = loop_visitor.ret_stmt {
            assert_eq!(stmt, expected);
        } else {
            panic!("DIDNT GET STATEMENT 1!");
        }

        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                let vect = Vec::new();
                for i in 0..5 {
                    if i < 10 {
                        vect.push(foo());
                        vect.push(foo());
                    }
                }
            }
        "#).unwrap();
        let expected: Stmt = syn::parse_str(r#"vect.push(foo());"#).unwrap();
        
        let foo = String::from("foo");
        let mut loop_visitor = LoopRewriter { target: &foo, done: false, ref_idx: get_ref_indices(&ast.sig), ret_stmt: None };
        loop_visitor.visit_item_fn_mut(&mut ast);

        if let Some(stmt) = loop_visitor.ret_stmt {
            assert_eq!(stmt, expected);
        } else {
            panic!("DIDNT GET STATEMENT 2!");
        }

        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() -> usize {
                let mut accumulator = 0;
                for i in 0..5 {
                    accumulator += foo();
                    accumulator += foo();
                }
            }
        "#).unwrap();
        let expected: Stmt = syn::parse_str(r#"accumulator += foo();"#).unwrap();
        
        let foo = String::from("foo");
        let mut loop_visitor = LoopRewriter { target: &foo, done: false, ref_idx: get_ref_indices(&ast.sig), ret_stmt: None };
        loop_visitor.visit_item_fn_mut(&mut ast);

        if let Some(stmt) = loop_visitor.ret_stmt {
            assert_eq!(stmt, expected);
        } else {
            panic!("DIDNT GET STATEMENT 3!");
        }

        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() -> usize {
                let mut accumulator = 0;
                for i in 0..5 {
                    acc = combined(acc, foo());
                    acc = combined(acc, foo());
                }
            }
        "#).unwrap();
        let expected: Stmt = syn::parse_str(r#"acc = combined(acc, foo());"#).unwrap();
        
        let foo = String::from("foo");
        let mut loop_visitor = LoopRewriter { target: &foo, done: false, ref_idx: get_ref_indices(&ast.sig), ret_stmt: None };
        loop_visitor.visit_item_fn_mut(&mut ast);

        if let Some(stmt) = loop_visitor.ret_stmt {
            assert_eq!(stmt, expected);
        } else {
            panic!("DIDNT GET STATEMENT 4!");
        }
    }

    #[test]
    fn test_ret_expr_all() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() -> usize {
                let mut accumulator = 0;
                for i in 0..5 {
                    while true  {
                        acc = combined(acc, foo());
                        acc = combined(acc, foo());
                    }
                }
            }
        "#).unwrap();
        // rewrite spawns
        let name_str = String::from("foo");
        let ref_idx = get_ref_indices(&ast.sig);
        let input_frame_str = String::from("crate::__Frame__::InputFoo");
        let input_frame_expr: Expr = syn::parse_str(&input_frame_str).unwrap();
        let mut spawn_replacer = SpawnReplace {
            count: None,
            insert_spawned: false,
            target: &name_str,
            ref_idx: &ref_idx,
            frame_name: input_frame_expr,
            ret_stmt: None,
        };
        spawn_replacer.visit_item_fn_mut(&mut ast);

        let expected: Stmt = syn::parse_str(r#"acc = combined(acc, foo());"#).unwrap();

        if let Some(stmt) = spawn_replacer.ret_stmt {
            println!("{:?}", stmt);
            assert_eq!(stmt, expected);
        } else {
            panic!("DIDNT GET STATEMENT 5!");
        }
    }

    #[test]
    fn test_e2e_nested1() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..10 {
                    some_fn();
                    if true {
                        foo();
                        foo();
                    }
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_nested2() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                if true {
                    for i in -1..50 {
                        some_fn();
                        foo();
                        foo();
                    }
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_loop() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..5 {
                    some_fn();
                    while true {
                        foo();
                        foo();
                    }
                }
                if true {
                    println!("hello");
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_non_literal() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in [1,2,3] {
                    some_fn();
                    foo();
                    foo();
                    another_fn();
                    foo();
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_literal() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 1..3 {
                    some_fn();
                    foo();
                    foo();
                    another_fn();
                    foo();
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_literal_long() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 1..30 {
                    some_fn();
                    foo();
                    foo();
                    another_fn();
                    foo();
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_no_target() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..5 {
                    some_fn();
                }
                if true {
                    println!("hello");
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_two_fors() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..5 {
                    some_fn();
                    foo();
                    foo();
                }
                for i in 0..10 {
                    some_fn();
                    foo();
                    foo();
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_mixed() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..5 {
                    some_fn();
                    foo();
                    foo();
                }
                foo();
                foo();
                while(true) {
                    foo();
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_spawned1() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..5 {
                    foo();
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_spawned2() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                for i in 0..10 {
                    foo();
                }
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

    #[test]
    fn test_e2e_spawned3() {
        let mut ast: syn::ItemFn = syn::parse_str(r#"
            fn foo() {
                foo();
                foo();
            }
        "#).unwrap();
        let _ = spawn_unknown(&mut ast);
        
        let code_string = quote::quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }

}