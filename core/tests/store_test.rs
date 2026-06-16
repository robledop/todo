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

use outlook_tasks_core::auth::classify_keyring_error;
use outlook_tasks_core::KeyringError;

#[test]
fn not_found_and_io_classify_as_unavailable() {
    let nf = oo7::Error::DBus(oo7::dbus::Error::NotFound("default".into()));
    assert!(matches!(classify_keyring_error(nf), KeyringError::Unavailable));

    let io = oo7::Error::DBus(oo7::dbus::Error::IO(std::io::Error::other("no bus")));
    assert!(matches!(classify_keyring_error(io), KeyringError::Unavailable));
}

use outlook_tasks_core::auth::Oo7TokenStore;

// Requires a running Secret Service provider (gnome-keyring/KWallet) and a
// session bus, so it runs by default locally but skips under CI, which has no
// keyring. CI runners conventionally set `CI`; honor that.
#[tokio::test]
async fn oo7_store_roundtrips_against_real_keyring() {
    if std::env::var_os("CI").is_some() {
        eprintln!("skipping oo7 keyring roundtrip: no Secret Service under CI");
        return;
    }
    let store = Oo7TokenStore::new("dev.robledop.OutlookTasks.test", "primary");
    store
        .save(&StoredToken { refresh_token: "live-rt".into(), account_id: "primary".into() })
        .await
        .unwrap();
    let loaded = store.load().await.unwrap().unwrap();
    assert_eq!(loaded.refresh_token, "live-rt");
    store.clear().await.unwrap();
    assert!(store.load().await.unwrap().is_none());
}
