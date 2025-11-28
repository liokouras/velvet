use std::{env, fs, path::{Path, PathBuf}, process::exit};
use syn;
use prettyplease;

// top-level function called by programmer's build.rs
// 1. searches for spawnable function signatures in provided files
// 2. defines app-specific Frame enum
// 3. defines app-specific steal() function
// 4. writes this to (new) file OUT_DIR/velvet_app.rs
pub fn generate(files: Vec<&str>) {
    // verify provided filepaths
    let mut filepaths = Vec::new();
    for file in files {
        let filepath = PathBuf::from(file);
        match filepath.try_exists() {
            Ok(true) => filepaths.push(filepath),
            Ok(false) => {
                println!("cargo::Error=provided file does not exist. Please check filepath and try again. Provided filepath: {:?}", filepath);
                exit(1);
            },
            Err(e) => {
                println!("cargo::Error=Existence of provided file could not be verified. Please check permissions on filepath and try again. 
                Provided filepath: {:?}. Error: {}", filepath, e);
                exit(1);
            }
        }
    }
    
    // find the spawnable functions in the provided files and create a 'database'
    let funcs = super::find_functions(filepaths);            
    let func_db = super::build_funcs_db(funcs);

    // use the database to write the custom enum and function
    let frame_enum = super::generate_frame_enum(&func_db);
    let steal_func = super::generate_steal_func(&func_db);

    // make velvet_app.rs file in output directory and write to it
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("velvet_app.rs");
    let code_string = format!("{}\n{}", frame_enum, steal_func);
    let verified_syntax_tree = syn::parse_file(&code_string).expect(&format!("Failed to parse generated code {}", &code_string));
    let pretty_code = prettyplease::unparse(&verified_syntax_tree);
    fs::write(
        &dest_path,
        pretty_code
    ).unwrap();
}