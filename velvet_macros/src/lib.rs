use proc_macro::TokenStream;

mod velvet_main;
mod spawnable;
mod spawn_known;
mod spawn_unknown;
mod sync;
mod par_iter;

#[proc_macro_attribute]
pub fn velvet_main(attr: TokenStream, item: TokenStream) -> TokenStream {
    velvet_main::build_velvet_main(attr, item)
}

#[proc_macro_attribute]
pub fn spawnable(attr: TokenStream, item: TokenStream) -> TokenStream {
    spawnable::build_spawnable(attr, item)
}

#[proc_macro_attribute]
pub fn par_iter(_attr: TokenStream, item: TokenStream) -> TokenStream {
    par_iter::build_par_iter(item)
}

// utility to convert from snake_case to PascalCase
fn snake_to_pascal(input: &String) -> String {
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