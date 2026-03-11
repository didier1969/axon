mod scanner;
mod parser;

fn main() {
    println!("Axon v2 Data Plane : Operational");
    
    let scanner = scanner::Scanner::new(".");
    let files = scanner.scan();
    println!("Scanned {} files", files.len());
}
