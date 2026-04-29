// Copyright (c) Didier Stadelmann. All rights reserved.
// NEXUS v10.7: Removed jemallocator. Using default system allocator for FFI/ONNX stability.
#![recursion_limit = "512"]

fn main() -> anyhow::Result<()> {
    axon_core::runtime_boot::run_indexer()
}
