fn main() {
    let paths = vec![
        "src/matrix_par.rs", 
    ];
    
    velvet::generate(paths);

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=./src/main.rs");
}