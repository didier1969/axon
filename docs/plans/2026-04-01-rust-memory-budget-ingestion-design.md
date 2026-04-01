# Rust Memory-Budget Ingestion Design

Date: 2026-04-01
Status: approved for planning

## Goal

Replace the current coarse `bulk / titan` ingestion behavior with a Rust-owned scheduler that admits work according to a dynamic memory budget, while removing the remaining Elixir ingestion authority except for visualization and operator-facing telemetry.

## Problem Statement

Axon has two known failure modes under WSL and similar constrained environments:

1. residual Elixir ingestion authority still participates in routing and pressure semantics
2. sudden waves of large files can over-admit work, grow RSS aggressively, trigger swap or writeback pressure, and freeze the host

The current `titan` mechanism is not enough:

- it is implemented through mixed Elixir and Rust decisions
- thresholds are not fully coherent across the system
- it is class-based rather than budget-based
- it protects partially, but not as a true memory admission controller

## Target Architecture

### 1. Rust as sole ingestion authority

All canonical ingestion decisions must live in Rust:

- file admission
- queueing
- concurrency
- memory protection
- degradation under pressure

Elixir remains limited to:

- visualization
- telemetry rendering
- operator actions relayed to Rust

### 2. Memory-budget scheduler

Instead of routing files by a fixed `normal vs titan` category, the scheduler computes whether a file may enter processing based on:

- file size
- parser/language class
- observed historical memory cost
- currently reserved in-flight memory
- runtime memory budget
- live pressure signals

The scheduler maintains:

- `memory_budget_bytes`
- `memory_reserved_bytes`
- `estimated_cost_bytes` per pending file
- `observed_cost_bytes` per completed file

A file is admitted only if:

`memory_reserved_bytes + estimated_cost_bytes <= effective_budget_bytes`

### 3. Progressive estimation

The initial estimate should not depend on exact historical knowledge being present.

The system should estimate cost in layers:

1. base estimate from file size
2. safety multiplier
3. parser/language correction factor
4. runtime-learned correction from observed historical cost

This allows the scheduler to start safe and become more precise over time.

### 4. Safety margin

The scheduler must preserve headroom for:

- the OS
- Docker and other local workloads
- MCP / SQL responsiveness
- temporary spikes from parser or embedding work

The target is not maximum throughput at all times.

The target is:

- no voluntary memory overcommit
- controlled degradation
- stable host behavior

## Scheduling Semantics

### Small files

Many small files may run concurrently if the total estimated in-flight cost remains below budget.

### Large files

A large file does not need a special semantic lane.

It naturally reduces concurrency because its estimate occupies a larger share of the budget.

### Hot path

Priority still matters.

The hot path should remain preferred, but it must still obey the memory budget.

Priority changes ordering, not the right to overcommit memory.

## Pressure Response

Pressure handling should be layered:

1. pause semantic/embedding work first
2. reduce claim rate for structural work
3. refuse new admissions when budget is exhausted
4. resume gradually after recovery

The scheduler should react not only to queue length, but also to:

- Axon RSS
- recent MCP/SQL latency
- memory budget exhaustion ratio

## Data Model

Each file should eventually carry, directly or derivatively:

- `size_bytes`
- `estimated_cost_bytes`
- `observed_peak_bytes` or equivalent measured cost
- `parser_kind`
- `priority`
- `status`

Observed cost should be stored in a reusable form so similar future files can be estimated more accurately.

## Migration Plan

### Phase 1

- keep the existing system working
- move remaining canonical routing decisions out of Elixir
- introduce memory-budget admission inside Rust
- keep `bulk / titan` only as a compatibility layer if needed internally

### Phase 2

- remove `titan` as the primary semantic model
- make scheduling depend on budget and observed cost
- reduce Elixir to visualization and operator relay only

## Guarantees and Non-Guarantees

### What this design can realistically improve

- far lower risk of RAM thrash
- fewer host freezes under WSL
- better parallelism on many small files
- better control during waves of large files
- less reliance on arbitrary skip behavior

### What it cannot honestly guarantee

- 100% absence of all memory incidents
- zero impact on throughput
- zero degradation in every pathological case

The honest contract is:

- stability first
- throughput second
- explicit degradation instead of host collapse

## Success Criteria

This design is successful if:

- Elixir no longer acts as canonical ingestion controller
- Rust alone decides admissions under pressure
- small files still ingest efficiently in parallel
- large-file waves no longer cause uncontrolled memory growth
- host responsiveness is preserved significantly better than today
