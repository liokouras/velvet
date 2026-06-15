fn main() {
    let paths = vec![
        "src/par_merge.rs", 
    ];
    
    velvet::generate(paths);

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=./src/par_merge.rs");
}