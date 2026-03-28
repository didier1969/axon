use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    println!("cargo:warning=OUT_DIR is {}", out_dir);
    
    let out_path = Path::new(&out_dir);
    let deps_dir = out_path.join("../../").canonicalize().unwrap(); 
    println!("cargo:warning=deps_dir is {}", deps_dir.display());
    
    let mut found = false;
    if let Ok(entries) = fs::read_dir(&deps_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.file_name().unwrap().to_string_lossy().starts_with("lbug-") {
                let yyjson_path = path.join("out/build/third_party/yyjson");
                println!("cargo:warning=Checking {}", yyjson_path.display());
                if yyjson_path.join("libyyjson.a").exists() {
                    println!("cargo:rustc-link-search=native={}", yyjson_path.display());
                    println!("cargo:rustc-link-lib=static=yyjson");
                    found = true;
                    println!("cargo:warning=Found libyyjson.a at {}", yyjson_path.display());
                }
            }
        }
    }
    
    if !found {
        println!("cargo:warning=Could not find libyyjson.a in any lbug output directory.");
        println!("cargo:rustc-link-lib=static=yyjson");
    }
}
