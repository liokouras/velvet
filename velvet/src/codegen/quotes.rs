use quote::quote;
use proc_macro2::{TokenStream, Span};

use super::func_finding::FuncEntry;

/*
    input: Vec<FuncEntry> where there is one FuncEntry per function to account for.
    a FuncEntry carries a function's name, qualified path, argument list and return type (if any)
    the argument list has already been augmented to change the 'self' type to an Arc<concrete-type>
    for associated methods.

    output: TokenStream which defines the Frame enum with:
    - 1 variant for every function, with arg type
    - 1 variant for every function that has a return type
    - Stolen variant: Stolen(Arc<Mutex<Option<Frame>>>)
    - implementation of Identifiable trait for Frame enum
*/
pub fn generate_frame_enum(funcs: &Vec<FuncEntry>) -> TokenStream {
    let mut enum_variants = Vec::new();
    let mut identifiable_branches = Vec::new();

    for func in funcs.iter() {
        let func_name = &func.name;
        let arg_types = &func.args;
        
        // convert to PascalCase to avoid warnings
        let pascal_func = snake_to_pascal(func_name.to_string());

        // create enum types
        let args_tuple = if arg_types.is_empty() {
            quote! ()
        } else {
            quote! { #(#arg_types),* }
        };

        let arg_variant_name = syn::Ident::new(&format!("Input{}", pascal_func), func_name.span());
        let arg_variant = quote!(#arg_variant_name(usize, #args_tuple));
        enum_variants.push(arg_variant);

        // if let Frame::Input___(uid, ..) = self { return *uid; }
        let identifiable_branch = quote!(if let __Frame__::#arg_variant_name(uid, ..) = self { return *uid; });
        identifiable_branches.push(identifiable_branch);

        if let Some(ret_ty) = &func.ret {
            let ret_variant_name = syn::Ident::new(&format!("Output{}", pascal_func), func_name.span());
            let ret_variant = quote!(#ret_variant_name(#ret_ty));
            enum_variants.push(ret_variant);
        }
    }
    
    // generate the full enum definition
    quote! {
        pub(crate) enum __Frame__ {
            Stolen(std::sync::Arc<std::sync::Mutex<Option<__Frame__>>>),
            #(#enum_variants),*
        }

        impl velvet::Identifiable for __Frame__ {
            fn get_id(&self) -> usize {
                #(#identifiable_branches)*
                return 0;
            }
        }
    }
}

/*
    input: Vec<FuncEntry> with functions to account for
    output: app-specific steal-function

    fn steal(worker: &mut VelvetWorker<Frame>) {
        let stealers = &worker.stealers;
        let len = stealers.len();
        let mut n = worker.get_random(len);

        let result_slot = Arc::new(Mutex::new(None));
        let mut lock = result_slot.lock().unwrap();
        for _ in 0..len {
            let maybe_frame = stealers[n].steal(Frame::Stolen(result_slot.clone()));

            if let Some(frame) = maybe_frame {
                match frame {
                    Frame::InputFuncX(_, a0, a1, a2, ...) => {
                        func_x(worker, a0, a1, a2, ...);
                        *lock = None;
                    },
                    Frame::InputFuncY(_, a0, ...) => {
                        let result = func_y(worker, a0, ...);
                        *lock = Frame::OutputFuncY(result);
                    },
                    _ => panic!("WRONG STOLEN WORK FRAME!"),
                }

                return;
            }
            n = (n + 1) % len;
        }
    }
*/
pub fn generate_steal_func(funcs: &Vec<FuncEntry>) -> TokenStream {
    let specific_steal_logic = generate_steal_logic(funcs);
    #[cfg(not(feature = "stats"))]
    quote!{
        fn __velvet_steal__(worker: &mut velvet::VelvetWorker<__Frame__>) {
            let stealers = &worker.stealers;
            let len = stealers.len();
            let mut n = worker.get_random(len);

            let result_slot = std::sync::Arc::new(std::sync::Mutex::new(None));
            let mut lock = result_slot.lock().unwrap();
            for _ in 0..len {
                let maybe_frame = stealers[n].steal(__Frame__::Stolen(result_slot.clone()));

                if let Some(frame) = maybe_frame {
                    match frame {
                        #specific_steal_logic
                    }
                    return;
                }
                n = (n+1)%len;
            }
        }
    }
    #[cfg(feature = "stats")]
    quote!{
        fn __velvet_steal__(worker: &mut velvet::VelvetWorker<__Frame__>) {
            let stealers = &worker.stealers;
            let len = stealers.len();
            let mut n = worker.get_random(len);

            let result_slot = std::sync::Arc::new(std::sync::Mutex::new(None));
            let mut lock = result_slot.lock().unwrap();
            for _ in 0..len {
                let maybe_frame = stealers[n].steal(__Frame__::Stolen(result_slot.clone()));

                if let Some(frame) = maybe_frame {
                    match frame {
                        #specific_steal_logic
                    }
                    worker.add_successful_steals(1);
                    return;
                }
                n = (n+1)%len;
            }
        }
    }
}

/*
    input: Vec<FuncEntry> where there is one FuncEntry per function to account for.
    output: the function-specific part of the steal logic as a TokenStream.
            for every function in funcs, have a match arm for Frame::FrameFunc(args...)
            and the logic to execute, namely calling the corresponding function with the arguments
            (in case of the augmented 'self' arg, add it as a reference to the Arc)
            and sending back the done-signal with return value (if any)
*/ 
fn generate_steal_logic(funcs: &Vec<FuncEntry>) -> TokenStream {
    let mut match_statements = Vec::new();

    for func in funcs {
        let func_name = &func.name;
        let func_path = &func.path;

        // convert to pascalcase to avoid warnings
        let pascal_func = snake_to_pascal(func_name.to_string());
        let frame_name = syn::Ident::new(&format!("Input{}", pascal_func), func_name.span());
        
        let ret_variable;
        let done;
        if let Some(_) = &func.ret {
            let res_frame = syn::Ident::new(&format!("Output{}", pascal_func), func_name.span());
            ret_variable = quote!(let result = );
            done = quote!(*lock = Some(__Frame__::#res_frame(result)));
        } else {
            ret_variable = quote!();
            done = quote!(*lock = None);
        }

        if !func.args.is_empty() {
            if func.has_selfarg {
                // the first argument is the selftype!
                let selftype = syn::Ident::new("a0", Span::call_site());
                let mut frame_args = Vec::new();
                let mut func_args = Vec::new();
                for i in 1..func.args.len() {
                    let arg_name = syn::Ident::new(&format!("a{}", i), Span::call_site());
                    frame_args.push(quote!(#arg_name));
                    if func.ref_args.contains(&i) {
                        func_args.push(quote!(&#arg_name))
                    } else {
                        func_args.push(quote!(#arg_name))
                    }
                }
                let frame_args_pattern = quote! { #(#frame_args),* };
                let func_args_pattern = quote! { #(#func_args),* };

                let stmt = quote! {
                    __Frame__::#frame_name(_, a0, #frame_args_pattern) => {
                        #ret_variable #selftype.#func_name(worker, #func_args_pattern);
                        #done;
                    }
                };
                match_statements.push(stmt);
            } else {
                let mut frame_args = Vec::new();
                let mut func_args = Vec::new();
                for i in 0..func.args.len() {
                    let arg_name = syn::Ident::new(&format!("a{}", i), Span::call_site());
                    frame_args.push(quote!(#arg_name));
                    if func.ref_args.contains(&i) {
                        func_args.push(quote!(&#arg_name))
                    } else {
                        func_args.push(quote!(#arg_name))
                    }
                }
                let frame_args_pattern = quote! { #(#frame_args),* };
                let func_args_pattern = quote! { #(#func_args),* };

                let stmt = quote! {
                    __Frame__::#frame_name(_, #frame_args_pattern) => {
                        #ret_variable #func_path(worker, #func_args_pattern);
                        #done;
                    },
                };
                match_statements.push(stmt);
            }
        } else {
            let stmt = quote! (
                __Frame__::#frame_name(_) => {
                    #ret_variable #func_path(worker);
                    #done;
                }
            );
            match_statements.push(stmt);
        }
    }

    quote! {
        #(#match_statements)*
        _ => panic!("WRONG STOLEN WORK FRAME!"),
    }
}

// utility to convert from snake_case to PascalCase
fn snake_to_pascal(input: String) -> String {
    let mut result = String::new();
    let mut uppercase_next = true;

    for (i, c) in input.chars().enumerate() {
        if c == '_' {
            uppercase_next = true;
        } else if uppercase_next {
            result.push(c.to_ascii_uppercase());
            uppercase_next = false;
        } else if i == 0 {
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }

    result
}