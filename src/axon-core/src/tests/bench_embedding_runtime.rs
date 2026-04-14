use crate::embedding_contract::MAX_LENGTH;
use ort::{
    ep,
    execution_providers::ExecutionProviderDispatch,
    session::{builder::GraphOptimizationLevel, Session},
    value::Value,
};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread::available_parallelism;
use std::time::Instant;
use tokenizers::{AddedToken, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

fn fastembed_cache_dir() -> PathBuf {
    if let Some(path) = std::env::var("FASTEMBED_CACHE_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(path).join("axon").join("fastembed");
    }
    PathBuf::from("/home/dstadel/.cache/axon/fastembed")
}

fn bge_large_snapshot_dir() -> PathBuf {
    let model_root = fastembed_cache_dir().join("models--Xenova--bge-large-en-v1.5");
    let snapshot_ref = model_root.join("refs").join("main");
    let snapshot = fs::read_to_string(&snapshot_ref)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", snapshot_ref.display()))
        .trim()
        .to_string();
    model_root.join("snapshots").join(snapshot)
}

fn env_bool(var: &str, default: bool) -> bool {
    std::env::var(var)
        .ok()
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no"
            )
        })
        .unwrap_or(default)
}

fn env_usize(var: &str) -> Option<usize> {
    std::env::var(var)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var(var)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn load_bge_large_tokenizer(snapshot_dir: &Path) -> Tokenizer {
    let tokenizer_json = snapshot_dir.join("tokenizer.json");
    let config_json = snapshot_dir.join("config.json");
    let special_tokens_map_json = snapshot_dir.join("special_tokens_map.json");
    let tokenizer_config_json = snapshot_dir.join("tokenizer_config.json");

    let config: serde_json::Value = serde_json::from_slice(
        &fs::read(&config_json)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", config_json.display())),
    )
    .unwrap_or_else(|err| panic!("failed to parse {}: {err}", config_json.display()));
    let special_tokens_map: serde_json::Value =
        serde_json::from_slice(&fs::read(&special_tokens_map_json).unwrap_or_else(|err| {
            panic!(
                "failed to read {}: {err}",
                special_tokens_map_json.display()
            )
        }))
        .unwrap_or_else(|err| {
            panic!(
                "failed to parse {}: {err}",
                special_tokens_map_json.display()
            )
        });
    let tokenizer_config: serde_json::Value =
        serde_json::from_slice(&fs::read(&tokenizer_config_json).unwrap_or_else(|err| {
            panic!("failed to read {}: {err}", tokenizer_config_json.display())
        }))
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", tokenizer_config_json.display()));

    let model_max_length = tokenizer_config["model_max_length"]
        .as_f64()
        .expect("tokenizer_config.json missing model_max_length")
        as usize;
    let max_length = MAX_LENGTH.min(model_max_length);
    let pad_id = config["pad_token_id"].as_u64().unwrap_or(0) as u32;
    let pad_token = tokenizer_config["pad_token"]
        .as_str()
        .expect("tokenizer_config.json missing pad_token")
        .to_string();

    let mut tokenizer = Tokenizer::from_file(&tokenizer_json)
        .unwrap_or_else(|err| panic!("{}: {err}", tokenizer_json.display()));
    tokenizer
        .with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            pad_token,
            pad_id,
            ..Default::default()
        }))
        .with_truncation(Some(TruncationParams {
            max_length,
            ..Default::default()
        }))
        .unwrap_or_else(|err| panic!("failed to configure tokenizer padding/truncation: {err}"));

    if let serde_json::Value::Object(root_object) = special_tokens_map {
        for value in root_object.values() {
            if let Some(content) = value.as_str() {
                tokenizer.add_special_tokens(&[AddedToken {
                    content: content.to_string(),
                    special: true,
                    ..Default::default()
                }]);
            } else if let (
                Some(content),
                Some(single_word),
                Some(lstrip),
                Some(rstrip),
                Some(normalized),
            ) = (
                value["content"].as_str(),
                value["single_word"].as_bool(),
                value["lstrip"].as_bool(),
                value["rstrip"].as_bool(),
                value["normalized"].as_bool(),
            ) {
                tokenizer.add_special_tokens(&[AddedToken {
                    content: content.to_string(),
                    special: true,
                    single_word,
                    lstrip,
                    rstrip,
                    normalized,
                }]);
            }
        }
    }

    tokenizer
}

fn load_bge_large_session(snapshot_dir: &Path) -> Session {
    let model_path = snapshot_dir.join("onnx").join("model.onnx");
    let intra_threads = env_usize("AXON_BENCH_INTRA_THREADS").unwrap_or_else(|| {
        available_parallelism()
            .expect("available_parallelism")
            .get()
    });
    let inter_threads = env_usize("AXON_BENCH_INTER_THREADS");
    let use_parallel_execution = env_bool("AXON_BENCH_PARALLEL_EXECUTION", false);
    let use_cuda = env_bool("AXON_BENCH_USE_CUDA", true);
    let optimized_model_path = env_path("AXON_BENCH_OPTIMIZED_MODEL_PATH");
    let profiling_path = env_path("AXON_BENCH_PROFILING_PATH");

    let mut builder = Session::builder()
        .expect("session builder")
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .expect("optimization level")
        .with_intra_threads(intra_threads)
        .expect("intra threads");

    if let Some(inter_threads) = inter_threads {
        builder = builder
            .with_inter_threads(inter_threads)
            .expect("inter threads");
    }

    if use_parallel_execution {
        builder = builder
            .with_parallel_execution(true)
            .expect("parallel execution");
    }

    if let Some(optimized_model_path) = optimized_model_path {
        builder = builder
            .with_optimized_model_path(&optimized_model_path)
            .expect("optimized model path");
    }

    if let Some(profiling_path) = profiling_path {
        builder = builder
            .with_profiling(&profiling_path)
            .expect("profiling path");
    }

    if use_cuda {
        let dispatch =
            ExecutionProviderDispatch::from(ep::CUDA::default().with_device_id(0).build())
                .error_on_failure();
        builder = builder
            .with_execution_providers(vec![dispatch])
            .expect("cuda execution provider");
    }

    builder
        .commit_from_file(&model_path)
        .unwrap_or_else(|err| panic!("failed to load {}: {err}", model_path.display()))
}

fn synthetic_texts() -> Vec<String> {
    let text_count = std::env::var("AXON_BENCH_TEXT_COUNT")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(128);
    let snippet = r#"
defmodule BookingSystem.Payments.InvoiceService do
  alias BookingSystem.Payments.Invoice
  alias BookingSystem.Repo

  def create_invoice!(attrs) do
    %Invoice{}
    |> Invoice.changeset(attrs)
    |> Repo.insert!()
  end

  def settle_invoice!(invoice_id, payment_id) do
    invoice = Repo.get!(Invoice, invoice_id)
    invoice
    |> Invoice.settlement_changeset(%{payment_id: payment_id, status: "paid"})
    |> Repo.update!()
  end
end
"#;

    (0..text_count)
        .map(|idx| {
            format!(
                "path: lib/payments/invoice_service_{idx}.ex\n{snippet}\n# repetition {}\n{}",
                idx % 7,
                snippet.repeat(2)
            )
        })
        .collect()
}

#[test]
#[ignore = "manual benchmark for tokenizer.encode_batch vs ort::Session::run"]
fn bench_bge_large_tokenizer_vs_ort_runtime() {
    let snapshot_dir = bge_large_snapshot_dir();
    let texts = synthetic_texts();

    let tokenizer = load_bge_large_tokenizer(&snapshot_dir);
    let mut session = load_bge_large_session(&snapshot_dir);

    let encode_started = Instant::now();
    let encodings = tokenizer
        .encode_batch(texts.iter().map(|text| text.as_str()).collect(), true)
        .expect("encode_batch");
    let encode_ms = encode_started.elapsed().as_millis() as u64;
    println!("encode_batch ms: {}", encode_ms);
    io::stdout().flush().expect("stdout flush");

    let encoding_length = encodings.first().expect("encodings").len();
    let batch_size = encodings.len();
    let max_size = encoding_length * batch_size;

    let mut ids_array = Vec::with_capacity(max_size);
    let mut mask_array = Vec::with_capacity(max_size);
    let mut type_ids_array = Vec::with_capacity(max_size);

    for encoding in &encodings {
        ids_array.extend(encoding.get_ids().iter().map(|x| *x as i64));
        mask_array.extend(encoding.get_attention_mask().iter().map(|x| *x as i64));
        type_ids_array.extend(encoding.get_type_ids().iter().map(|x| *x as i64));
    }

    let mut session_inputs = ort::inputs![
        "input_ids" => Value::from_array(([batch_size, encoding_length], ids_array)).expect("input_ids value"),
        "attention_mask" => Value::from_array(([batch_size, encoding_length], mask_array)).expect("attention_mask value"),
    ];

    if session
        .inputs()
        .iter()
        .any(|input| input.name() == "token_type_ids")
    {
        session_inputs.push((
            "token_type_ids".into(),
            Value::from_array(([batch_size, encoding_length], type_ids_array))
                .expect("token_type_ids value")
                .into(),
        ));
    }

    let run_started = Instant::now();
    let outputs = session.run(session_inputs).expect("session.run");
    let run_ms = run_started.elapsed().as_millis() as u64;
    let output_count = outputs.into_iter().count();
    let profiling_output = if std::env::var("AXON_BENCH_PROFILING_PATH").is_ok() {
        Some(session.end_profiling().expect("end_profiling"))
    } else {
        None
    };

    println!("\n--- [ BGE LARGE TOKENIZER VS ORT BENCH ] ---");
    println!("Snapshot dir: {}", snapshot_dir.display());
    println!("Texts: {}", texts.len());
    println!("Encoding length: {}", encoding_length);
    println!("session.run ms: {}", run_ms);
    if let Some(profiling_output) = profiling_output {
        println!("profiling file: {}", profiling_output);
    }
    println!("Output tensors: {}", output_count);
    assert!(output_count > 0, "expected at least one output tensor");
}

#[test]
fn test_bench_env_bool() {
    unsafe {
        std::env::remove_var("AXON_BENCH_TEST_BOOL");
        assert!(env_bool("AXON_BENCH_TEST_BOOL", true));
        assert!(!env_bool("AXON_BENCH_TEST_BOOL", false));
        std::env::set_var("AXON_BENCH_TEST_BOOL", "false");
        assert!(!env_bool("AXON_BENCH_TEST_BOOL", true));
        std::env::set_var("AXON_BENCH_TEST_BOOL", "1");
        assert!(env_bool("AXON_BENCH_TEST_BOOL", false));
        std::env::remove_var("AXON_BENCH_TEST_BOOL");
    }
}

#[test]
fn test_bench_env_usize_and_path() {
    unsafe {
        std::env::remove_var("AXON_BENCH_TEST_USIZE");
        std::env::remove_var("AXON_BENCH_TEST_PATH");
        assert_eq!(env_usize("AXON_BENCH_TEST_USIZE"), None);
        assert_eq!(env_path("AXON_BENCH_TEST_PATH"), None);

        std::env::set_var("AXON_BENCH_TEST_USIZE", "16");
        std::env::set_var("AXON_BENCH_TEST_PATH", "/tmp/optimized-bge-large.onnx");
        assert_eq!(env_usize("AXON_BENCH_TEST_USIZE"), Some(16));
        assert_eq!(
            env_path("AXON_BENCH_TEST_PATH"),
            Some(PathBuf::from("/tmp/optimized-bge-large.onnx"))
        );

        std::env::set_var("AXON_BENCH_TEST_USIZE", "0");
        std::env::set_var("AXON_BENCH_TEST_PATH", "   ");
        assert_eq!(env_usize("AXON_BENCH_TEST_USIZE"), None);
        assert_eq!(env_path("AXON_BENCH_TEST_PATH"), None);

        std::env::remove_var("AXON_BENCH_TEST_USIZE");
        std::env::remove_var("AXON_BENCH_TEST_PATH");
    }
}
