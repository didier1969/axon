// Copyright (c) Didier Stadelmann. All rights reserved.

//! Bug-fix coverage for the admission-vs-vectorization budget decoupling.
//!
//! Before the fix, queue admission and worker claim shared a single
//! `reserved_bytes` pool. At bootstrap a large pending backlog drove the
//! exhaustion ratio above the `claim_policy` pause threshold, deadlocking
//! the pipeline because workers could not pop while admission held the
//! whole budget.
//!
//! The fix splits accounting into two pools:
//!  * `queued_bytes` — admitted but not yet claimed
//!  * `inflight_bytes` — currently being processed
//!
//! `MemoryBudgetSnapshot.exhaustion_ratio` derives from `inflight_bytes`
//! only, so admission no longer pauses the worker pool.

use crate::queue::QueueStore;

#[test]
fn test_admission_does_not_inflate_exhaustion_ratio_before_workers_claim() {
    let temp = tempfile::tempdir().unwrap();
    let queue = QueueStore::with_memory_budget(64, 6_000_000);

    for idx in 0..3 {
        let path = temp.path().join(format!("admit_{}.rs", idx));
        std::fs::write(&path, vec![b'x'; 1024]).unwrap();
        queue
            .push(
                path.to_string_lossy().as_ref(),
                0,
                &format!("admit-{}", idx),
                0,
                0,
                false,
            )
            .unwrap();
    }

    let snapshot = queue.memory_budget_snapshot();
    assert!(
        snapshot.reserved_bytes > 0,
        "queued admissions should still be accounted for"
    );
    assert_eq!(
        snapshot.inflight_bytes, 0,
        "no worker has claimed yet, inflight must be zero"
    );
    assert!(
        snapshot.exhaustion_ratio < 0.01,
        "exhaustion_ratio must reflect inflight only, got {}",
        snapshot.exhaustion_ratio
    );
}

#[test]
fn test_pop_moves_bytes_from_queued_to_inflight() {
    let temp = tempfile::tempdir().unwrap();
    let queue = QueueStore::with_memory_budget(10, 6_000_000);
    let path = temp.path().join("claim.rs");
    std::fs::write(&path, vec![b'x'; 4 * 1024]).unwrap();
    queue
        .push(path.to_string_lossy().as_ref(), 0, "claim", 0, 0, false)
        .unwrap();

    let pre_pop = queue.memory_budget_snapshot();
    assert!(pre_pop.reserved_bytes > 0);
    assert_eq!(pre_pop.inflight_bytes, 0);

    let task = queue.pop().expect("task admitted should be poppable");
    let post_pop = queue.memory_budget_snapshot();
    assert!(
        post_pop.inflight_bytes > 0,
        "pop must claim the reservation into inflight"
    );
    assert_eq!(
        post_pop.inflight_bytes, pre_pop.reserved_bytes,
        "all queued bytes for the popped task should move into inflight"
    );

    queue.mark_done(&task, None).unwrap();
    let post_done = queue.memory_budget_snapshot();
    assert_eq!(post_done.reserved_bytes, 0);
    assert_eq!(post_done.inflight_bytes, 0);
}
