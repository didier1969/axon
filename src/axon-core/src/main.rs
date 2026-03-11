mod parser;
mod scanner;

use parser::{Parser, python::PythonParser};
use rayon::prelude::*;
use std::time::Instant;
use std::fs;
use std::env;

fn main() {
    println!("Axon v2 Data Plane : Operational");
    
    let args: Vec<String> = env::args().collect();
    let root = if args.len() > 1 { &args[1] } else { "." };

    let start = Instant::now();
    let scanner = scanner::Scanner::new(root);
    let files = scanner.scan();
    println!("Found {} files to process in {}", files.len(), root);

    let python_parser = PythonParser::new();

    let total_symbols: usize = files.par_iter().map(|path| {
        if let Some(ext) = path.extension() {
            if ext == "py" {
                if let Ok(content) = fs::read_to_string(path) {
                    let result = python_parser.parse(&content);
                    return result.symbols.len();
                }
            }
        }
        0
    }).sum();

    let duration = start.elapsed();
    println!("Processed {} symbols in {:?}", total_symbols, duration);
}
