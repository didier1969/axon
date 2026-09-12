use crate::ist_snapshot::cluster_pubsub::{
    ClusterPubSub, InProcessBroadcastPubSub, ShardMutationEvent,
};

#[tokio::test]
async fn test_in_process_broadcast_pubsub_flow() {
    let pubsub = InProcessBroadcastPubSub::new(16);
    let mut rx1 = pubsub.subscribe();
    let mut rx2 = pubsub.subscribe();

    let event = ShardMutationEvent {
        project_code: "AXO".to_string(),
        shard_id: 2,
        mutation_epoch: 42,
        invalidated_files: vec!["src/lib.rs".to_string(), "src/main.rs".to_string()],
    };

    pubsub
        .publish(event.clone())
        .await
        .expect("publish event should succeed");

    let received1 = rx1.recv().await.expect("rx1 receives event");
    let received2 = rx2.recv().await.expect("rx2 receives event");

    assert_eq!(received1, event);
    assert_eq!(received2, event);
    assert_eq!(received1.shard_id, 2);
    assert_eq!(received1.invalidated_files.len(), 2);
}

#[test]
fn test_pubsub_event_serialization_roundtrip() {
    let event = ShardMutationEvent {
        project_code: "PRJ".to_string(),
        shard_id: 7,
        mutation_epoch: 999,
        invalidated_files: vec!["app/model.py".to_string()],
    };

    let serialized = serde_json::to_string(&event).expect("serialize event");
    let deserialized: ShardMutationEvent =
        serde_json::from_str(&serialized).expect("deserialize event");

    assert_eq!(event, deserialized);
}
