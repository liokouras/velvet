use std::{env, fs, path::{Path, PathBuf}};
use syn;
use prettyplease;

// top-level function called by programmer's build.rs
// 1. searches for spawnable function signatures in provided files
// 2. defines app-specific Frame enum
// 3. defines app-specific steal() function
// 4. writes this to (new) file OUT_DIR/velvet_app.rs
pub fn generate(files: Vec<&str>) {
    // verify provided filepaths
    // errors are reported to cargo (see report_error) and checking continues, so that all errors are reported at once
    let mut filepaths = Vec::new();
    for file in files {
        let filepath = PathBuf::from(file);
        match filepath.try_exists() {
            Ok(true) => filepaths.push(filepath),
            Ok(false) => {
                super::report_error(&format!("velvet::generate: file {:?} (listed in build.rs) does not exist. Please check the path; it is relative to the package root (the directory containing Cargo.toml).", filepath));
            },
            Err(e) => {
                super::report_error(&format!("velvet::generate: could not check whether file {:?} (listed in build.rs) exists: {}. Please check the path and its permissions.", filepath, e));
            }
        }
    }

    // find the spawnable functions in the provided files and create a 'database'
    let funcs = super::find_functions(filepaths);
    let func_db = super::build_funcs_db(funcs);

    // after any error, cargo fails the build once the build script finishes; do not generate code
    if super::errors_reported() {
        return;
    }

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

    // set envr variable with spawnable function names
    let spawnables: Vec<String> = func_db.into_iter().map(|entry| entry.name.to_string()).collect();
    let spawnables = spawnables.join(",");
    println!("cargo:rustc-env=SPAWNABLES={}", spawnables);

}