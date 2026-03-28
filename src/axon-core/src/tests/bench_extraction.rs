use std::time::Instant;
use std::sync::Arc;
use crate::parser;

#[test]
fn bench_pure_parsing_speed() {
    let content = "defmodule Test do\n  def hello, do: :ok\n  def world, do: :error\nend\n".repeat(10);
    let path = std::path::Path::new("test.ex");
    let parser = parser::get_parser_for_file(path).unwrap();
    
    let iterations = 1000;
    let start = Instant::now();
    
    for _ in 0..iterations {
        let _res = parser.parse(&content);
    }
    
    let duration = start.elapsed();
    let per_file = duration.as_micros() as f64 / iterations as f64;
    let files_per_sec = 1_000_000.0 / per_file;
    
    println!("\n--- [ PURE PARSING BENCHMARK ] ---");
    println!("Total duration for {} iterations: {:?}", iterations, duration);
    println!("Time per file: {:.2} µs", per_file);
    println!("Theoretical throughput: {:.2} files/sec (on 1 core)", files_per_sec);
    println!("Theoretical throughput (14 cores): {:.2} files/sec", files_per_sec * 14.0);
}
