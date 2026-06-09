use outlook_tasks_core::auth::{InMemoryTokenStore, StoredToken, TokenStore};

#[tokio::test]
async fn in_memory_store_roundtrips() {
    let store = InMemoryTokenStore::default();
    assert!(store.load().await.unwrap().is_none());

    store
        .save(&StoredToken { refresh_token: "rt1".into(), account_id: "primary".into() })
        .await
        .unwrap();

    let loaded = store.load().await.unwrap().unwrap();
    assert_eq!(loaded.refresh_token, "rt1");
    assert_eq!(loaded.account_id, "primary");

    store.clear().await.unwrap();
    assert!(store.load().await.unwrap().is_none());
}
