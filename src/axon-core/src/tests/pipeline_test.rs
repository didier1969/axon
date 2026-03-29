use std::sync::Arc;
use std::io::Write;
use tokio::sync::broadcast;

use crate::graph::GraphStore;
use crate::queue::QueueStore;
use crate::worker::{WorkerPool, DbWriteTask};

#[test]
fn test_full_pipeline_loop() {
    println!("[TEST] Starting End-to-End Pipeline Loop Test...");
    
    // 1. Setup GraphStore in memory
    println!("[TEST] Step 1: Init GraphStore...");
    let graph = Arc::new(GraphStore::new(":memory:").expect("Failed to init GraphStore"));
    
    // 2. Setup QueueStore
    println!("[TEST] Step 2: Init QueueStore...");
    let queue = Arc::new(QueueStore::new(10));
    
    // 3. Create a mock Elixir file with proper extension
    println!("[TEST] Step 3: Create mock Elixir file...");
    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join("test_file_axon.ex");
    let mut file = std::fs::File::create(&file_path).unwrap();
    writeln!(file, "defmodule Test do\n  def hello, do: :ok\nend").unwrap();
    let path = file_path.to_string_lossy().to_string();
    
    // 4. Pre-insert file into DB as pending (simulating scanner)
    println!("[TEST] Step 4: Bulk insert file...");
    graph.bulk_insert_files(&[(path.clone(), "test_proj".to_string(), 100, 12345)]).expect("Failed to insert file");
    
    // 5. Setup Worker infrastructure
    println!("[TEST] Step 5: Init infrastructure...");
    let (results_tx, _results_rx) = broadcast::channel::<String>(100);
    let (db_sender, db_receiver) = crossbeam_channel::unbounded::<DbWriteTask>();
    
    // 6. Push task to Queue
    println!("[TEST] Step 6: Push task...");
    queue.push(&path, 12345, "test_trace", 0, 0, false).expect("Failed to push task");
    
    // 7. Run Worker logic for one task (Manual single-threaded execution)
    println!("[TEST] Step 7: Pop and Process task...");
    let task = queue.pop().expect("Queue should have 1 task");
    WorkerPool::process_one_task(0, task, &db_sender, &results_tx);
    
    // 8. Verify Worker sent a write task to the Actor (with timeout to avoid hangs)
    println!("[TEST] Step 8: Receive write task...");
    let write_task = db_receiver.recv_timeout(std::time::Duration::from_secs(5))
        .expect("Worker should have sent a write task within 5s");
    
    // 9. Manually run the Writer Actor logic for this batch
    println!("[TEST] Step 9: Committing to Database...");
    let batch = vec![write_task];
    graph.insert_file_data_batch(&batch).expect("Failed to commit batch");
    
    // 10. VERIFY TRUTH IN DATABASE
    println!("[TEST] Step 10: VERIFY TRUTH IN DATABASE");
    let indexed_count = graph.query_count("SELECT count(*) FROM File WHERE status = 'indexed'").expect("Query failed");
    assert_eq!(indexed_count, 1, "File should be marked as indexed");
    
    let symbol_count = graph.query_count("SELECT count(*) FROM Symbol").expect("Query failed");
    println!("[TEST] Symbols found: {}", symbol_count);
    assert!(symbol_count > 0, "At least one symbol (the module) should be extracted");
    
    // Verify 9 columns integrity
    let symbols_json = graph.query_json("SELECT name, kind, is_public, tested, is_nif, is_unsafe FROM Symbol").expect("Query failed");
    println!("[TEST] Extracted Symbols: {}", symbols_json);
    assert!(symbols_json.contains("Test"), "Module name 'Test' should be present in extracted symbols");
    
    // Clean up
    let _ = std::fs::remove_file(&file_path);
    println!("[TEST] SUCCESS: Pipeline loop is functional.");
}
