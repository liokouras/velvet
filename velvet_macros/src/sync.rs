use proc_macro2::Span;
use syn::{parse::{Parser, Result}, parse_quote, visit::{self, Visit}, visit_mut::{self, VisitMut},
    Arm, Block, Expr::{self, *}, Ident, ItemFn, Stmt, StmtMacro, Type};
use quote::quote;
use std::collections::HashSet;

use crate::spawnable::get_ref_indices;
use super::spawnable::SyncInput;

/*
    CASES:
    - 'with' vs 'without' return values
        In the with-return-values case, need a sensitivity-list and insert syncs before sensitivity use
        In the without-return-values case, need syncs before all points of return (after a spawn)
    - 'numbered' vs 'looped' spawns
        In the 'numbered' spawns case, all spawns are numbered, and can sync on specific spawns
        In the 'looped' spawns case, no UIDs; count & checkpoint logic. for multi-block, have a checkpoint reset
*/

pub(super) fn sync(ast: &mut ItemFn, sync_input: SyncInput, shared_vars: &Vec<syn::LitStr>) -> Result<()> {
    let input_case = gen_input_frame_line(&ast.sig);
    let output_case = gen_output_frame_line(&ast.sig);
    let sync_logic = gen_sync_logic(&input_case, &output_case);

    let mut counted_spawns = false;

    // generate the sync stmts and sensitivity list
    let mut sync_stmts_singleblock: Vec<Vec<Stmt>>;
    let mut sync_stmts_multiblock = Vec::new();
    let mut sensitivities = Vec::new(); // for positioning
    let vars = if shared_vars.len() > 0 {
        let maybe_vars = shared_vars.iter()
            .map(|litstr| litstr.parse::<Ident>())
            .collect::<Result<HashSet<_>>>();
        maybe_vars.expect("Ensure arguments to 'shared' are valid variable identifiers")
    } else { HashSet::new() };
    sensitivities.push(vars);
    match sync_input {
        SyncInput::Known(num_calls, stmts) => {
            // NO LOOPS; all spawns are numbered
            counted_spawns = true;
            sync_stmts_singleblock = Vec::with_capacity(num_calls-1);
            if let Some(stmts) = stmts {
                // case of contentful sync statements
                for count in 0..(num_calls-1) {
                    let mut sync_stmts = Vec::new();
                    let uid_ident = Ident::new(&format!("__{}__", count), Span::call_site());
                    let worker_sync: Stmt = parse_quote!( let __SYNC__ = __worker__.sync(#uid_ident); );

                    sync_stmts.push(worker_sync.clone());
                    sync_stmts.push(sync_logic.clone());
                    if let Some(ref stmt) = stmts[count] {
                        sync_stmts.push(stmt.clone());
                        // collect vars - TODO: is it too coarse grained to collect all vars?
                        let mut collector = VarCollector{ vars: HashSet::new() };
                        collector.visit_stmt(stmt);
                        sensitivities.push(collector.vars);
                    } else { sensitivities.push( HashSet::new() )}

                    sync_stmts_singleblock.push(sync_stmts);
                }
            } else {
                // case of num_calls-many void sync statements [could still be sensitive to shared vars!]
                for count in 0..(num_calls-1) {
                    let uid_ident = Ident::new(&format!("__{}__", count), Span::call_site());
                    let worker_sync: Stmt = parse_quote!( let __SYNC__ = __worker__.sync(#uid_ident); );
                    sync_stmts_singleblock.push(Vec::from([worker_sync.clone(), sync_logic.clone()]));
                }
            }
        },
        SyncInput::Unknown(stmt) => {
            // LOOPS; count + checkpoint logic
            let while_loop: Stmt = if let Some(stmt) = stmt {
                // case of contentful sync
                // collect vars - TODO: is it too coarse grained to collect all vars?
                let mut collector = VarCollector{ vars: HashSet::new() };
                collector.visit_stmt(&stmt);
                sensitivities.push(collector.vars);

                parse_quote!(
                    while __count__ > 0 {
                        let __SYNC__ = __worker__.sync(__checkpoint__ + __count__);
                        #sync_logic
                        #stmt
                        __count__ -= 1;
                    };
                )
            } else {
                // case of void sync statements
                parse_quote!(
                    while __count__ > 0 {
                        let __SYNC__ = __worker__.sync(__checkpoint__ + __count__);
                        #sync_logic
                        __count__ -= 1;
                    };
                )
            };

            // sync stmt
            sync_stmts_singleblock = vec![Vec::from([while_loop.clone()])];
            sync_stmts_multiblock = vec![while_loop, parse_quote!(__checkpoint__ = __worker__.get_seq();)];
        }
    };

    // place sync-tags
    let has_retval = if let syn::ReturnType::Default = ast.sig.output { false } else { true };
    place_sync_markers(&mut ast.block, sensitivities, counted_spawns, has_retval);
    // remove spawn-tags:
    let mut remover = TagRemover{ tag_name: "spawned", keep_first_tag: false, count: 0 };
    remover.visit_block_mut(&mut ast.block);

    // replace sync-tags
    if counted_spawns {
        let mut replacer = IndexedReplacer { sync_logic: &sync_stmts_singleblock };
        replacer.visit_block_mut(&mut ast.block);
    } else {
        let sync_logic = if remover.count > 1 { &sync_stmts_multiblock } else { &sync_stmts_singleblock[0] };
        let mut replacer = Replacer { sync_logic };
        replacer.visit_block_mut(&mut ast.block);
    }

    Ok(())
}

/* ~~~ positioning ~~~
    CASES:
    - NO RET VAL: sync before function returns [implicit and explicit returns]
    - SIDE EFFECTS: user must specify which variables to be sensitive to; sync as in RET-VAL
    - RET VAL:
    -- RET VAL USED IN SAME SCOPE: sync before first use
    -- RET VAL USED IN LOOP or DIFFERENT SCOPES
    --- scopes are separate from spawns: sync before (first) scope
    --- scopes are same as spawns (so have spawn-after-sync):
        multiple syncs, in each scope, before first-use.
        + RESET SEQ CHECKPOINT! (bc decrementing count)

    ARGS:
        block: the function body..
        sensitivities: Vec of set of variables to be sensitive to
            -  sensitivities[0] is the programmer-provided set of shared global vars; might be of size zero
            - for non-void functions:
                sensitivities will be of length num-spawns + 1 (the +1 is the shared vars at idx 0)
                this is to match the spawn-id with the corresponding sensitivities (+1 offset bc of above)
            - for void functions: sensitivities is of length exactly 1
        counted: true => syncing on specific spawns. false => syncing on loop-spawns

*/
fn place_sync_markers(block: &mut Block, sensitivities: Vec<HashSet<Ident>>, counted: bool, ret_val: bool) {
    if counted {
        // we have to keep track of spawns in scope
        let mut visitor = BlockFinder { sensitivities: &sensitivities };
        visitor.visit_block_mut(block);
    } else {
        // we are in an un-numbered context [spawns in loops!]
        // basically just need to place a sync before every sensitivity-use
        // and keep track of whether we have seen another spawn or not
        let flat_sensitivities: Vec<Ident> = sensitivities.into_iter().flat_map(|set| set.into_iter()).collect();
        if place_sync_markers_loops(block, &flat_sensitivities, false) {
            // have an unsynced spawn; insert into block
            let len = block.stmts.len();
            if len > 0 {
                if !ret_val { block.stmts.push(parse_quote! { sync!(); }); }
                else {
                    let last_stmt = &block.stmts[len-1];
                    // check for explicit/implicit return
                    if matches!(last_stmt, Stmt::Expr(Return(_) , _)) ||
                        matches!(last_stmt, Stmt::Expr(_, None)) ||
                        matches!(last_stmt, Stmt::Macro(syn::StmtMacro { semi_token: None, .. })) {
                        // sync BEFORE return
                        block.stmts.insert(len-1, parse_quote! { sync!(); });
                    } else {
                        block.stmts.push(parse_quote! { sync!(); });
                    }
                }
            }
        }
    }
}

/*
    places a sync! before a statement with sensitivity-use
    syncs must be in same scope as they were spawned, bc of compiler-checked var-initialisation...
    As a block is being analysed:
    - collect set of spawns 'in scope' (at the same level)
    - if there is an expression, check if it uses any of the in-scope spawns
    -- if yes, insert a sync before the expression + remove (LIFO MANNER) those spawns from set
    -- if no, do nothing..

    - recurse into all expressions
*/
struct BlockFinder<'ast> { sensitivities: &'ast Vec<HashSet<Ident>>}
impl<'ast> VisitMut for BlockFinder <'ast> {
    fn visit_block_mut(&mut self, block: &mut Block) {
        place_sync_markers_numbered(block, self.sensitivities);
        syn::visit_mut::visit_block_mut(self, block);
    }
}

fn place_sync_markers_numbered(block: &mut Block, sensitivities: &Vec<HashSet<Ident>>) {
    let mut new_stmts = Vec::new();
    let mut spawns_in_scope= Vec::new();

    for stmt in block.stmts.iter_mut() {
        match stmt {
            Stmt::Macro(StmtMacro{ mac, .. }) => {
                if mac.path.is_ident("spawned") {
                    let tokens = mac.tokens.clone();
                    if let Ok(Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(int), .. })) = syn::parse2::<Expr>(tokens) {
                        if let Ok(val) = int.base10_parse::<usize>() {
                            spawns_in_scope.push(val);
                        }
                    }
                }
            },

            Stmt::Expr(expr, _) => {
                handle_sensitivity_scope(expr, &mut new_stmts, &mut spawns_in_scope, sensitivities);
            },

            Stmt::Local(syn::Local { init: Some(local_init), .. }) => {
                // let var = expr else diverge_expr
                handle_sensitivity_scope(&local_init.expr, &mut new_stmts, &mut spawns_in_scope, sensitivities);
                if let Some((_, diverge_expr)) = &local_init.diverge {
                    handle_sensitivity_scope(&*diverge_expr, &mut new_stmts, &mut spawns_in_scope, sensitivities);
                }
            }

            _ => (),
        }
        new_stmts.push(stmt.clone());
    }

    // if there are still spawns in scope here, must sync!
    if spawns_in_scope.len() > 0 {
        spawns_in_scope.reverse();
        let synctag = quote! { sync!(#(#spawns_in_scope),*); };

        let len = new_stmts.len();
        if len > 0 {
            let last_stmt = &mut new_stmts[len-1];
            // check for explicit/implicit return
            if matches!(last_stmt, Stmt::Expr(Return(_) , _)) ||
                matches!(last_stmt, Stmt::Expr(_, None)) ||
                matches!(last_stmt, Stmt::Macro(syn::StmtMacro { semi_token: None, .. })) {
                // sync BEFORE return
                new_stmts.insert(len-1, parse_quote! { #synctag });
            } else {
                new_stmts.push(parse_quote! { #synctag });
            }
        }
    }

    block.stmts = new_stmts;
}

fn handle_sensitivity_scope(expr: &Expr, new_stmts: &mut Vec<Stmt>, spawns_in_scope: &mut Vec<usize>, sensitivities: &Vec<HashSet<Ident>>) -> bool {
    let matches = has_sensitivity_numbered(&expr, &spawns_in_scope, sensitivities);
    if matches.len() > 0 {
        // sync on the needed spawns now (in same scope!)
        // given LIFO semantics, must sync on all spawns in the match + the ones after
        let idx = matches.iter().filter_map(|&matche| {
                spawns_in_scope.iter().position(|&spawn| spawn == matche)
            }).min().unwrap();
        // remove synced ids from spawns-in-scope in case there are some left which we don't have to sync yet
        let mut ids_to_sync = spawns_in_scope.split_off(idx);
        ids_to_sync.reverse();

        // create sync tags
        let synctag = quote! { sync!(#(#ids_to_sync),*); };
        new_stmts.push(parse_quote! { #synctag });
        true
    } else { false }
}

struct VarCollector { vars: HashSet<Ident> }
impl <'stmt> Visit <'stmt> for VarCollector {
    fn visit_expr_path(&mut self, expr_path: &'stmt syn::ExprPath) {
        if let Some(ident) = expr_path.path.get_ident() {
            if ident !=  "__SYNC_RES__" {self.vars.insert(ident.clone());}
        }
    }

    fn visit_pat_ident(&mut self, pat_ident: &'stmt syn::PatIdent) {
        let ident = &pat_ident.ident;
        if ident !=  "__SYNC_RES__" {
            self.vars.insert(ident.clone());
        }
    }

    fn visit_expr_method_call(&mut self, node: &'stmt syn::ExprMethodCall) {
        // skip method name (node.method); visit ags
        self.visit_expr(&node.receiver);
        for arg in &node.args {
            self.visit_expr(arg);
        }
    }

    fn visit_expr_call(&mut self, node: &'stmt syn::ExprCall) {
        // skip func name; visit args
        for arg in &node.args {
            self.visit_expr(arg);
        }
    }

    fn visit_pat(&mut self, pat: &'stmt syn::Pat) {
        visit::visit_pat(self, pat);
    }
}

fn has_sensitivity_numbered(expr: &Expr, ids: &Vec<usize>, sensitivities: &Vec<HashSet<Ident>>) -> Vec<usize> {
    let mut collector = VarCollector{ vars: HashSet::new() };
    collector.visit_expr(expr);

    let mut in_scope_spawns = Vec::new();
    if sensitivities.iter().all(|sens_list| sens_list.is_empty()) {
        in_scope_spawns
    } else {
        for id in ids {
            // sensitivity-indexing is off-by-one because of shared variables in pos 0
            if sensitivities[*id + 1].iter().any(|ident| collector.vars.contains(ident)) {
                in_scope_spawns.push(*id);
            }
        }
        // it is more nuanced than this: only care about reads (incl distinguishing between VAR.load and VAR.store)!! TODO
        if sensitivities[0].iter().any(|ident| collector.vars.contains(ident)) {
            in_scope_spawns.push(0);
        }

        in_scope_spawns
    }
}

// places a sync! before a statement with sensitivity-use
fn place_sync_markers_loops(block: &mut Block, sensitivities: &Vec<Ident>, mut seen_spawn: bool) -> bool {
    let mut new_stmts = Vec::new();

    for stmt in block.stmts.iter_mut() {
        match stmt {
            Stmt::Macro(StmtMacro{ mac, .. }) => {
                if mac.path.is_ident("spawned") {
                    seen_spawn = true;
                }
            },

            Stmt::Expr(expr, _) => {
                match expr {
                    Loop(_) | While(_) | ForLoop(_) => {
                        if has_sensitivity(&expr, sensitivities) && seen_spawn {
                            // can place tag directly before stmt
                            new_stmts.push(parse_quote! { sync!(); });
                            seen_spawn = false;
                        }
                        // check if it has a spawn
                        seen_spawn = seen_spawn || contains_spawn(expr);
                    },

                    Block(blockexpr) => { seen_spawn = place_sync_markers_loops(&mut blockexpr.block, sensitivities, seen_spawn); }

                    Return(_) => {
                        if seen_spawn {
                            new_stmts.push(parse_quote! { sync!(); });
                            seen_spawn = false;
                        }
                    }

                    _ => {
                        // use helper for the rest
                        let (spawn, sync) = handle_expr(expr, sensitivities, seen_spawn);
                        if sync && seen_spawn {
                            new_stmts.push(parse_quote! { sync!(); });
                        }
                        seen_spawn = spawn;
                    }
                }
            },

            Stmt::Local(syn::Local { init: Some(local_init), .. }) => {
                if has_sensitivity(&local_init.expr, sensitivities) && seen_spawn {
                    new_stmts.push(parse_quote! { sync!(); });
                    seen_spawn = false;
                } else if let Some((_, diverge_expr)) = &mut local_init.diverge {
                    let expr_mut = &mut **diverge_expr;
                    // use helper for the diverge-expression
                    let (spawn, sync) = handle_expr(expr_mut, sensitivities, seen_spawn);
                    if sync && seen_spawn {
                        new_stmts.push(parse_quote! { sync!(); });
                    }
                    seen_spawn = spawn;
                }
            }

            _ => (),
        }
        new_stmts.push(stmt.clone());
    }
    block.stmts = new_stmts;
    seen_spawn
}

// returns (bool, bool): there is an unmatched spawn in this expr, and there must be a sync before this expr
fn handle_expr(expr: &mut Expr, sensitivities: &Vec<Ident>, mut seen_spawn: bool) -> (bool, bool) {
    let mut ret_val = false;
    match expr {
        If(syn::ExprIf { cond, then_branch, else_branch, .. }) => {
            // if the condition has a sensitivity, the sync must be before it; return true
            if seen_spawn && has_sensitivity(&cond, sensitivities) { seen_spawn = false; ret_val = true; }
            
            // analyse the block (will place syncs inside if necessary)
            let nested_outstanding_spawn = place_sync_markers_loops(then_branch, sensitivities, seen_spawn);
            
            if let Some((_, else_expr)) = else_branch {
                (seen_spawn, ret_val) = handle_expr(else_expr, sensitivities, seen_spawn);
            }
            seen_spawn = seen_spawn || nested_outstanding_spawn;
        },

        Match(syn::ExprMatch { expr: match_expr, arms, .. }) => {
            // if the match-statement has a sensitivity, the sync must be before it; return true
            if seen_spawn && has_sensitivity(match_expr, sensitivities) { seen_spawn = false; ret_val = true; }

            let (mut acc_spawn, mut acc_sync) = (false, false);
            
            for arm in arms {
                let (spawn, sync) = handle_expr(&mut *arm.body, sensitivities, seen_spawn); 
                acc_spawn = acc_spawn || spawn;
                acc_sync = acc_sync || sync;
            }

            seen_spawn = seen_spawn || acc_spawn;
            ret_val = ret_val || acc_sync;
        },

        While(_) | ForLoop(_) | Loop(_)=> {
            ret_val = seen_spawn;
            seen_spawn = contains_spawn(expr);
        },

        Block(block) => { seen_spawn = place_sync_markers_loops(&mut block.block, sensitivities, seen_spawn); }

        _ => {
            if seen_spawn && has_sensitivity(expr, sensitivities) { ret_val = true; }
            seen_spawn = contains_spawn(expr);
        }
    }
    return (seen_spawn, ret_val);
}

fn has_sensitivity(expr: &Expr, sensitivities: &Vec<Ident>) -> bool {
    let mut collector = VarCollector{ vars: HashSet::new() };
    collector.visit_expr(expr);
    sensitivities.iter().any(|id| collector.vars.contains(id))
}

fn contains_spawn(expr: &Expr) -> bool {
    struct ContainsSpawn { found_spawned: bool, }
    impl <'stmt> Visit <'stmt> for ContainsSpawn {
        fn visit_stmt(&mut self, stmt: &'stmt Stmt) {
            if self.found_spawned { return; }
            if let Stmt::Macro(StmtMacro{mac, ..}) = stmt {
                if mac.path.is_ident("spawned") {
                    self.found_spawned = true;
                    return;
                }
            }
            visit::visit_stmt(self, stmt);
        }
    }
    let mut visitor = ContainsSpawn { found_spawned: false };
    visitor.visit_expr(expr);
    return visitor.found_spawned;
}
struct TagRemover { tag_name: &'static str, keep_first_tag: bool, count: usize }
impl VisitMut for TagRemover {
    fn visit_block_mut(&mut self, node: &mut Block) {
        visit_mut::visit_block_mut(self, node);
        
        let mut new_stmts = Vec::new();
        for stmt in &node.stmts {
            if let Stmt::Macro(syn::StmtMacro{ mac, .. }) = stmt {
                if mac.path.is_ident(self.tag_name) { 
                    if self.keep_first_tag { self.keep_first_tag = false; }
                    else { self.count+=1; continue; }
                }
            }
            new_stmts.push(stmt.clone());
        }
        node.stmts = new_stmts;  
    }
}

struct Replacer<'a> { sync_logic: &'a Vec<Stmt> }
impl <'a> VisitMut for Replacer <'a> {
    fn visit_block_mut(&mut self, node: &mut Block) {
        visit_mut::visit_block_mut(self, node);
        
        let mut new_stmts = Vec::new();

        for stmt in node.stmts.iter_mut() {
            if let Stmt::Macro(StmtMacro{mac, ..}) = stmt {
                if mac.path.is_ident("sync") {
                    new_stmts.extend(self.sync_logic.iter().cloned());
                    continue;
                }
            }
            new_stmts.push(stmt.clone());
        }
        node.stmts = new_stmts;
    }
}

struct IndexedReplacer<'a> { sync_logic: &'a Vec<Vec<Stmt>> }
impl <'a> VisitMut for IndexedReplacer <'a> {
    fn visit_block_mut(&mut self, node: &mut Block) {
        visit_mut::visit_block_mut(self, node);
        
        let mut new_stmts = Vec::new();
        
        for stmt in node.stmts.iter_mut() {
            if let Stmt::Macro(StmtMacro{mac, ..}) = stmt {
                let mut list = Vec::new();
                if mac.path.is_ident("sync") {
                    let parser = syn::punctuated::Punctuated::<Expr, syn::token::Comma>::parse_terminated;
                    if let Ok(exprs) = parser.parse2(mac.tokens.clone()) {
                        for expr in exprs {
                            if let Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(int), .. }) = expr {
                                if let Ok(val) = int.base10_parse::<usize>() {
                                    list.push(val);
                                }
                            }
                        }
                    }
                    for idx in list {
                        new_stmts.extend(self.sync_logic[idx].iter().cloned());
                    }
                    continue; // skips macro
                }
            }
            new_stmts.push(stmt.clone());
        }
        node.stmts = new_stmts;
    }
}

/* WANT: 
    ' crate::__Frame__::InputFuncname(_, a0, a1, ...) => funcname(worker, a0, a1, ...), '
    + check for adding '&'s and whether a0 is a receiver!
*/
fn gen_input_frame_line(sig: &syn::Signature) -> Arm {
    let func = &sig.ident;
    let pascal_func = super::snake_to_pascal(&func.to_string());
    let frame_name = Ident::new(&format!("Input{}", pascal_func), sig.ident.span());

    let mut ref_idx = get_ref_indices(sig);
    // if there is a receiver, add 1 to all refs! 
    // (bc 2 was subtracted in function for spawn-insertion functionality...)
    if sig.receiver().is_some() { ref_idx.iter_mut().for_each(|x| *x += 1); }

    let mut frame_args = Vec::new();
    let mut func_args = Vec::new();

    // -1 to account for worker-arg
    for i in 0..sig.inputs.len()-1 {
        let arg_name = Ident::new(&format!("a{}", i), Span::call_site());
        frame_args.push(quote!(#arg_name)); 

        if ref_idx.contains(&i) { // +1 to account for worker-arg
            func_args.push(quote!(&#arg_name));
        } else {
            func_args.push(quote!(#arg_name));
        }
    }

    let lhs: syn::Pat = parse_quote!(crate::__Frame__::#frame_name(_ #(,#frame_args)*));

    let rhs: Expr = if sig.receiver().is_some() {
        // call a0.func(__worker__, a1, ...);
        let func_args_slice = &func_args[1..];
        parse_quote!(a0.#func(__worker__ #(,#func_args_slice)*))
    } else {
        // call func(__worker__, a0, ....)
        parse_quote!(#func(__worker__ #(,#func_args)*))
    };

    parse_quote!( #lhs => #rhs, )
}

/* WANT: 
    case of void func: { break; }
    case of ret-val: if let Some(crate::__Frame__::OutputFUNCNAME(result)) = *_value { break result }
                     else { panic!("WRONG STOLEN RESULT FRAME!"); } 
*/ 
fn gen_output_frame_line(sig: &syn::Signature) -> Block {
    match sig.output {
        syn::ReturnType::Default => parse_quote!( { break; } ),
        syn::ReturnType::Type(_, ref boxed_type) => {
            match **boxed_type {
                Type::Tuple(ref tuple) if tuple.elems.is_empty() => {
                    parse_quote!( { break; } )
                },
                _ => {
                    let func = &sig.ident;
                    let pascal_func = super::snake_to_pascal(&func.to_string());
                    let frame_name = Ident::new(&format!("Output{}", pascal_func), sig.ident.span());

                    parse_quote!( {
                        if let Some(crate::__Frame__::#frame_name(result)) = (*_value).take(){ 
                            break result 
                        } else { 
                            panic!("WRONG STOLEN RESULT FRAME!"); 
                        }
                    })
                }
            }
        }
    }
}

/* WANT:
    let __SYNC_RES__ = match __SYNC__ {
        Frame::InputXXX(_, a0, a1, a2, a3 ...) => XXX(worker, a0, a1, a2, a3 ...),
        Frame::Stolen(ptr) => {
            let mut try_lock = ptr.try_lock();
            loop {
                if let Ok(_value) = try_lock { **EITHER BREAK OR USE _value** }
                else {
                    worker.steal();
                    try_lock = ptr.try_lock();
                }
            }
        },
        _ => panic!("WRONG FRAME POPPED!"),
    };
*/
fn gen_sync_logic(input_case: &Arm, output_case: &Block) -> Stmt {
    #[cfg(not(feature = "stats"))]
    return parse_quote!(
        let __SYNC_RES__ = match __SYNC__ {
        #input_case
        crate::__Frame__::Stolen(ptr) => {
            let mut try_lock = ptr.try_lock();
            loop {
                if let Ok(mut _value) = try_lock #output_case 
                else {
                    __worker__.steal();
                    try_lock = ptr.try_lock();
                }
            }
        },
        _ => panic!("WRONG FRAME POPPED!"),
    };);

    #[cfg(feature = "stats")]
    return parse_quote!(
        let __SYNC_RES__ = match __SYNC__ {
        #input_case
        crate::__Frame__::Stolen(ptr) => {
            let mut try_lock = ptr.try_lock();
            __worker__.add_stolen_jobs(1);
            loop {
                if let Ok(mut _value) = try_lock #output_case
                else {
                    __worker__.steal();
                    try_lock = ptr.try_lock();
                }
            }
        },
        _ => panic!("WRONG FRAME POPPED!"),
    };);
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::*;
    use quote::ToTokens;
    use syn::visit::Visit;

    #[test]
    fn test_input_arm() {
        let no_arg: ItemFn = syn::parse_str(r#"
            fn foo(worker: worker) -> usize {}
        "#).unwrap();
        let arm_no_arg = gen_input_frame_line(&no_arg.sig);
        let arm_no_arg_exp: Arm = syn::parse_str(r#"
            crate::__Frame__::InputFoo(_) => foo(__worker__),
        "#).unwrap();

        let plain: ItemFn = syn::parse_str(r#"
            fn foo(worker: worker, x: usize, y: i32, z: bool) -> usize {}
        "#).unwrap();
        let arm_plain = gen_input_frame_line(&plain.sig);
        let arm_plain_exp: Arm = syn::parse_str(r#"
            crate::__Frame__::InputFoo(_, a0, a1, a2) => foo(__worker__, a0, a1, a2),
        "#).unwrap();

        let refs: ItemFn = syn::parse_str(r#"
            fn foo(worker: worker, x: usize, y: Arc<i32>, z: &bool) -> usize {}
        "#).unwrap();
        let arm_refs = gen_input_frame_line(&refs.sig);
        let arm_refs_exp: Arm = syn::parse_str(r#"
            crate::__Frame__::InputFoo(_, a0, a1, a2) => foo(__worker__, a0, a1, &a2),
        "#).unwrap();

        let reciever: ItemFn = syn::parse_str(r#"
            fn foo(self, worker: worker, x: usize, y: Arc<i32>, z: &bool) -> usize {}
        "#).unwrap();
        let arm_reciever = gen_input_frame_line(&reciever.sig);
        let arm_receiver_exp: Arm = syn::parse_str(r#"
            crate::__Frame__::InputFoo(_, a0, a1, a2, a3) => a0.foo(__worker__, a1, a2, &a3),
        "#).unwrap();

        println!("{}", arm_no_arg.to_token_stream());
        println!("{}", arm_no_arg_exp.to_token_stream());

        println!("{}", arm_plain.to_token_stream());
        println!("{}", arm_plain_exp.to_token_stream());

        println!("{}", arm_refs.to_token_stream());
        println!("{}", arm_refs_exp.to_token_stream());

        println!("{}", arm_reciever.to_token_stream());
        println!("{}", arm_receiver_exp.to_token_stream());

        assert_eq!(arm_no_arg, arm_no_arg_exp);
        assert_eq!(arm_plain, arm_plain_exp);
        assert_eq!(arm_refs, arm_refs_exp);
        assert_eq!(arm_reciever, arm_receiver_exp);
    }

    fn gen_sync_logic_stub(ast: ItemFn) -> Stmt{
        let input_case: Arm = gen_input_frame_line(&ast.sig);
        let output_case: Block = gen_output_frame_line(&ast.sig);
        gen_sync_logic(&input_case, &output_case)
    }

    #[test]
    fn test_sync_logic() {
        let no_arg: ItemFn = syn::parse_str(r#"
            fn foo(workerarg: worker) {}
        "#).unwrap();
        let no_arg_sync = gen_sync_logic_stub(no_arg);

        let plain: ItemFn = syn::parse_str(r#"
            fn foo(workerarg: worker, x: usize, y: i32, z: bool) -> usize {}
        "#).unwrap();
        let plain_sync = gen_sync_logic_stub(plain);

        let refs: ItemFn = syn::parse_str(r#"
            fn foo(workerarg: worker, x: usize, y: Arc<i32>, z: &bool) -> () {}
        "#).unwrap();
        let refs_sync = gen_sync_logic_stub(refs);


        let reciever: ItemFn = syn::parse_str(r#"
            fn foo(self, workerarg: worker, x: usize, y: Arc<i32>, z: &bool) -> usize {}
        "#).unwrap();
        let reciever_sync = gen_sync_logic_stub(reciever);

        let tuple_res: ItemFn = syn::parse_str(r#"
            fn foo(self, workerarg: worker, x: usize, y: Arc<i32>, z: &bool) -> (usize, usize) {}
        "#).unwrap();
        let tuple_res_sync = gen_sync_logic_stub(tuple_res);

        println!("{}", no_arg_sync.to_token_stream());
        println!("{}", plain_sync.to_token_stream());
        println!("{}", refs_sync.to_token_stream());
        println!("{}", reciever_sync.to_token_stream());

        println!("{}", tuple_res_sync.to_token_stream());

    }

    #[test]
    fn test_var_collection() {
        let examples = [
            "let x = y;",
            "let baz = foo(bar);",
            "foo(bar);",
            "my_vec.push((a, b));",
            "let (x, y) = tup;",
            "let MyStruct { a, b } = s;",
        ];

        for code in &examples {
            let stmt: Stmt = syn::parse_str(code).unwrap();
            let mut collector = VarCollector{ vars: HashSet::new() };
            collector.visit_stmt(&stmt);
            let idents: Vec<syn::Ident>= collector.vars.into_iter().collect();
            println!("{:<40} => {:?}", code, idents);
        }
    }

    #[test]
    fn test_sync_input_stmt() {
        let mut ast: ItemFn = syn::parse_str(r#"
            fn fib(n: u32) -> u64 {
                if n < THRESHOLD {
                    return fib_seq(n);
                } else {
                    let r1 = fib(n-1);
                    let r2 = fib(n-2);
                    let res = r1 + r2;
                    return res;
                }
            }"#).unwrap();

        // add VelvetWorker as an argument to the function
        spawnable::add_worker(&mut ast);

        // adds the 'spawn' logic
        let sync_input = {
            match spawn_unknown::spawn_unknown(&mut ast) {
                Err(_) => panic!("error"),
                Ok(res) => res,
            }
        };

        match sync_input {
            SyncInput::Known(len, vec) => {
                println!("GOT KNOWN INPUT, len = {}", len);
                if let Some(vec) = vec {
                    println!("GOT A VEC! len = {}", vec.len());
                    for idx in 0..vec.len() {
                        match vec[idx] {
                            Some(ref s) => println!("STMT at IDX {} = {:?}", idx, s),
                            None => println!("NO STMT at IDX {}!", idx),
                        }
                    }

                } else {
                    println!("GOT NO VEC!");
                }

            },
            SyncInput::Unknown(_stmt) => {

            }

        };
    }

    #[test]
    fn test_tagging() {
        let mut ast: ItemFn = syn::parse_str(r#"
            fn fib(n: u32) -> u64 {
                if n < THRESHOLD {
                    return fib_seq(n);
                } else {
                    let r1 = fib(n-1);
                    let r2 = fib(n-2);

                    for _ in 0..n {
                        while true {
                            let x = r1 + r2;
                        }
                    }
                    
                    let r3 = fib(n-1);
                    let r4 = fib(n-2);
                    for _ in 0..n {
                        while true {
                            let x = r3+ r4;
                        }
                    }

                    return res;
                }
            }"#).unwrap();

        // add VelvetWorker as an argument to the function
        spawnable::add_worker(&mut ast);
        
        // adds the 'spawn' logic
        let sync_input = {
            match spawn_unknown::spawn_unknown(&mut ast) {
                Err(_) => panic!("error"),
                Ok(res) => res,
            }
        };

        let _ = sync(&mut ast, sync_input, &Vec::new());

        let code_string = quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);

        /*
        let mut locator = SyncLocator { 
            loop_depth: 0,
            sync_blocks: HashSet::new(),
            spawn_blocks: HashSet::new(),
            sync_loops: HashSet::new(),
            in_loop: None,
            current_block: None,
            total_syncs: 0,
            total_spawns: 0,
            spawn_after_sync: false,
        };

        locator.visit_item_fn(&ast);

        println!{"total syncs: {}, spawns: {}, spawn-after-sync: {}  \n loop depth: {} \n sync-blocks: {}\n spawn-blocks: {} \n sync-loops: {}\n ",
        locator.total_syncs, locator.total_spawns, locator.spawn_after_sync, locator.loop_depth, locator.sync_blocks.len(), locator.spawn_blocks.len(), locator.sync_loops.len() };

        assert!(locator.sync_blocks.is_disjoint(&locator.spawn_blocks));
        assert_eq!(locator.sync_loops.len(), 2);*/
    }

    #[test]
    fn test_looping1() {

        let mut ast: ItemFn = syn::parse_str(r#"
            fn fib(n: u32) -> u64 {
                let mut vec = Vec::new();
                if n == 3 { return n; }
                if n < 10 {
                        for _ in 0..n{
                            vec.push(fib(n-1));
                        }
                }

                if n < 10 {
                    return n;
                }

                let x = vec.len();
                x
            }"#).unwrap();

        // add VelvetWorker as an argument to the function
        spawnable::add_worker(&mut ast);
        
        // adds the 'spawn' logic
        let sync_input = {
            match spawn_unknown::spawn_unknown(&mut ast) {
                Err(_) => panic!("error"),
                Ok(res) => res,
            }
        };

        let _ = sync(&mut ast, sync_input, &Vec::new());

        let code_string = quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);
    }


    #[test]
    fn test_looping2() {

        let mut ast: ItemFn = syn::parse_str(r#"
            fn fib(n: u32) {
                let mut vec = Vec::new();
                if n == 3 { return n; }
                if cond {
                    for _ in 0..10{
                        vec.push(fib(n-1));
                    }
                }
            }"#).unwrap();

        // add VelvetWorker as an argument to the function
        spawnable::add_worker(&mut ast);
        
        // adds the 'spawn' logic
        let sync_input = {
            match spawn_unknown::spawn_unknown(&mut ast) {
                Err(_) => panic!("error"),
                Ok(res) => res,
            }
        };

        let counted  =  match sync_input {
            SyncInput::Known(_, _) => true,
            SyncInput::Unknown(_) => false,
        };

        place_sync_markers(&mut ast.block, Vec::new(), counted, false);


        let code_string = quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("{}", pretty);

        let _ = sync(&mut ast, sync_input, &Vec::new());

        
    }

    #[test]
    fn test_void_fn(){
        let mut ast: ItemFn = syn::parse_str(r#"
            fn fib_void1(n: u64) {
                if n < THRESHOLD {
                    return;
                } else {
                    fib_void1(n-1);
                    fib_void1(n-2);
                    fib_void1(n-3);
                    return;
                }
            }"#).unwrap();

        // add VelvetWorker as an argument to the function
        spawnable::add_worker(&mut ast);
        
        // adds the 'spawn' logic
        let sync_input = {
            // case of known number of recursive calls
            let num_calls: usize = 4;
            match spawn_known::spawn_known(&mut ast, num_calls) {
                Err(_) => panic!("error"),
                Ok(res) => res,
            }
        };

        let _ = sync(&mut ast, sync_input, &Vec::new());
        let code_string = quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("RESULT \n {}", pretty);

    }

    #[test]
    fn test_spawnsyncspawn_fn(){
        let mut ast: ItemFn = syn::parse_str(r#"
            fn fib(n: usize) -> usize {
                if n < THRESHOLD {
                    return n;
                }
                let bazi = fib(n);
                let e = fib(n);
                let d = fib(n); 
                let c = fib(n); 
                let a = fib(n);
                let b = fib(n);
                

                if true {
                    let y = fib(n);
                    let x = fib(n);
                    let z = fib(n);
                    return a + x;
                } else {
                    let i = c;
                    return a + b;
                }
                
                return d;
            }"#).unwrap();

        // add VelvetWorker as an argument to the function
        spawnable::add_worker(&mut ast);
        
        // adds the 'spawn' logic
        let sync_input = {
            // case of known number of recursive calls
            match spawn_unknown::spawn_unknown(&mut ast) {
                Err(_) => panic!("error"),
                Ok(res) => res,
            }
        };

        let _ = sync(&mut ast, sync_input, &Vec::new());
        let code_string = quote!(#ast).to_string();
        let file = syn::parse_file(&code_string).unwrap();
        let pretty = prettyplease::unparse(&file);
        println!("RESULT \n {}", pretty);
    }
}
