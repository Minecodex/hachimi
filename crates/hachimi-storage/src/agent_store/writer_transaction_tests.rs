use super::*;
use std::time::Duration;

#[tokio::test]
async fn separate_store_writers_reserve_before_reading_and_preserve_wal_readers() {
    let root = tempfile::tempdir().expect("temporary database directory");
    let path = root.path().join("agent.sqlite3");
    let first = AgentStore::connect(&path).await.expect("first store");
    let second = AgentStore::connect(&path).await.expect("second store");
    sqlx::query("CREATE TABLE writer_probe (value INTEGER NOT NULL)")
        .execute(first.pool())
        .await
        .expect("probe table");
    sqlx::query("INSERT INTO writer_probe VALUES (0)")
        .execute(first.pool())
        .await
        .expect("probe value");

    let mut transaction = first.begin_write().await.expect("first writer");
    let initial: i64 = sqlx::query_scalar("SELECT value FROM writer_probe")
        .fetch_one(&mut *transaction)
        .await
        .expect("first snapshot");
    assert_eq!(initial, 0);
    let concurrent_reader: i64 = sqlx::query_scalar("SELECT value FROM writer_probe")
        .fetch_one(second.pool())
        .await
        .expect("WAL reader remains available");
    assert_eq!(concurrent_reader, 0);

    let competing_store = second.clone();
    let mut competing_writer = tokio::spawn(async move {
        let mut transaction = competing_store.begin_write().await.expect("second writer");
        let current: i64 = sqlx::query_scalar("SELECT value FROM writer_probe")
            .fetch_one(&mut *transaction)
            .await
            .expect("second snapshot");
        assert_eq!(current, 1, "writer must read after the earlier commit");
        sqlx::query("UPDATE writer_probe SET value = value + 1")
            .execute(&mut *transaction)
            .await
            .expect("second update");
        transaction.commit().await.expect("second commit");
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut competing_writer)
            .await
            .is_err(),
        "another writer must wait while the first owns the transaction"
    );
    sqlx::query("UPDATE writer_probe SET value = value + 1")
        .execute(&mut *transaction)
        .await
        .expect("first update");
    transaction.commit().await.expect("first commit");
    competing_writer.await.expect("writer task");
    let value: i64 = sqlx::query_scalar("SELECT value FROM writer_probe")
        .fetch_one(first.pool())
        .await
        .expect("both writes persisted");
    assert_eq!(value, 2);
    first.pool().close().await;
    second.pool().close().await;
}
