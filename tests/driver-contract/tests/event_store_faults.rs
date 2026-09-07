//! EventStore fault-injection gates（PR10 commit 5 §10/§11）：
//! 磁盘满、busy 竞争、管理面隔离。生产接缝仅 `StoreFaults`（默认不注入）；
//! busy 锁走真实 SQLite 语义（busy_timeout=5000 冻结值）。

mod common;
mod event_common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use event_common::{EventTestSource, SimulatorEventSource, commit, event_batch, rows_of, tmp_db};
use mesa_event_store::{EventStore, EventStoreError, StoreFaults};

fn faulted_store(
    tag: &str,
    fail_at: u64,
    sticky: bool,
) -> (std::path::PathBuf, Arc<StoreFaults>, EventStore) {
    let db = tmp_db(tag);
    let _ = std::fs::remove_file(&db);
    let faults = Arc::new(StoreFaults::new(fail_at, sticky));
    let store = EventStore::open_with_faults(&db, Arc::clone(&faults)).unwrap();
    (db, faults, store)
}

/// 注入 SQLITE_FULL：失败批整批回滚（0 新行），错误走 Fatal 归一路径
///（ingress 映射为 `EVENT_STORE_UNAVAILABLE`，由管理面测试锁定）。
#[tokio::test]
async fn injected_full_rolls_back_batch() {
    let (db, _faults, store) = faulted_store("full", 3, false);
    let ep = "hd-fault-full";
    commit(
        &store,
        ep,
        event_batch(1, 1, 1, vec![event_common::base_record("f-1")]),
    )
    .await
    .unwrap();
    commit(
        &store,
        ep,
        event_batch(1, 1, 2, vec![event_common::base_record("f-2")]),
    )
    .await
    .unwrap();
    let err = commit(
        &store,
        ep,
        event_batch(
            1,
            1,
            3,
            vec![
                event_common::base_record("f-3a"),
                event_common::base_record("f-3b"),
            ],
        ),
    )
    .await
    .expect_err("第 3 个 commit 必须注入失败");
    assert!(
        matches!(err, EventStoreError::Fatal(_)),
        "注入故障必须走 Fatal 路径，got {err:?}"
    );
    let rows = rows_of(&store, ep);
    assert_eq!(rows.len(), 2, "失败批必须整批回滚");
    let _ = std::fs::remove_file(&db);
}

/// 短暂 busy（< timeout）：等待后提交成功，不丢不失败。
#[tokio::test]
async fn sqlite_busy_short_waits_and_commits() {
    let db = tmp_db("busy-short");
    let _ = std::fs::remove_file(&db);
    let store = EventStore::open(&db).unwrap();
    // 旁路连接占排他锁 1s（< busy_timeout 5000ms）
    let locker = rusqlite::Connection::open(&db).unwrap();
    locker
        .execute_batch("PRAGMA busy_timeout=5000; BEGIN EXCLUSIVE;")
        .unwrap();
    let ep = "hd-busy-short";
    let store = Arc::new(store);
    let h = tokio::spawn({
        let store = Arc::clone(&store);
        async move {
            commit(
                &store,
                ep,
                event_batch(1, 1, 1, vec![event_common::base_record("b-1")]),
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    locker.execute_batch("COMMIT;").unwrap();
    let res = h.await.unwrap().expect("短暂 busy 后必须提交成功");
    assert_eq!(res.inserted.len(), 1);
    assert_eq!(rows_of(&store, ep).len(), 1);
    let _ = std::fs::remove_file(&db);
}

/// 长时间 busy（> timeout）：显式失败，不静默丢弃。
#[tokio::test]
async fn sqlite_busy_past_timeout_fails_closed() {
    let db = tmp_db("busy-long");
    let _ = std::fs::remove_file(&db);
    let store = EventStore::open(&db).unwrap();
    let locker = rusqlite::Connection::open(&db).unwrap();
    locker
        .execute_batch("PRAGMA busy_timeout=5000; BEGIN EXCLUSIVE;")
        .unwrap();
    let ep = "hd-busy-long";
    // 占锁 8s（> 5s timeout）：commit 必须显式 Fatal，而不是挂起/丢弃
    let res = tokio::time::timeout(
        Duration::from_secs(15),
        commit(
            &store,
            ep,
            event_batch(1, 1, 1, vec![event_common::base_record("c-1")]),
        ),
    )
    .await
    .expect("commit 必须在 timeout 后返回");
    let err = res.expect_err("超 timeout 必须失败");
    assert!(
        matches!(err, EventStoreError::Fatal(_)),
        "busy 超时必须走 Fatal 路径，got {err:?}"
    );
    assert!(rows_of(&store, ep).is_empty(), "失败不得入库");
    locker.execute_batch("ROLLBACK;").unwrap();
    let _ = std::fs::remove_file(&db);
}

/// 管理面隔离：故障 Store 下事件 endpoint 失败可观测（计数+写停滞+可恢复），
/// 同一 manager 的 data-only endpoint 全程不受影响。
/// NOTE：`is_running` 在 Lost 后同 task 内重连，恒为 true——"不伪装 RUNNING"
/// 的可观测形式是失败计数 + 写停滞 + 恢复，而非运行态翻转。
/// 回归卫士：本测试曾抓到 teardown 复 poll 已消费 ingress handle 的 panic
///（"JoinHandle polled after completion" 直接杀死 endpoint task 使重连失效）；
/// 绿即证明 Lost → 重连 → 恢复链路完整。
#[tokio::test]
async fn faulted_endpoint_fails_observably_while_data_only_survives() {
    common::init_log();
    // SAFETY：测试进程内单值设置（同 ensure_pki_dir 模式）；重连退避固定 1s，
    // 否则恢复窗口可能短于 backoff 睡眠（30s 档）造成误红。
    unsafe {
        std::env::set_var("MESA_RECONNECT_FAST", "1");
    }
    let db = tmp_db("isolate");
    let _ = std::fs::remove_file(&db);
    let faults = Arc::new(StoreFaults::new(3, true));
    let store = Arc::new(EventStore::open_with_faults(&db, Arc::clone(&faults)).unwrap());
    let services = mesa_event_store::EventServices::new(
        store.clone(),
        mesa_event_store::EventHub::new(mesa_event_store::EVENT_HUB_CAPACITY),
    );
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("drivers"),
    ));
    mgr.set_event_services(Arc::clone(&services));

    // A：事件 endpoint（alarm，每 Start 若干 commits；第 3 个 commit 起全失败）
    let mut src_a = SimulatorEventSource::attach(
        "hd-fault-a",
        Arc::clone(&mgr),
        store.clone(),
        Arc::clone(&services),
        db.clone(),
    );
    // B：data-only endpoint（无事件任务，永不碰 Store）
    mgr.start_endpoint(mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "hd-fault-b".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: vec![common::poll_task(
            "d",
            50,
            serde_json::json!({"points": [{"key":"k.counter","kind":"counter"}]}),
        )],
        event_tasks: vec![],
    })
    .unwrap();

    src_a.start_endpoint().await;
    // 等失败计数出现（attempt 已真实失败，不是"看起来 RUNNING 就没事"）
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let f = services
            .diagnostics
            .ingress_store_failures_total
            .load(Ordering::Relaxed);
        if f >= 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "60s 内无 store 失败计数"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // 写停滞：2s 内 A 行数纹丝不动（失败不伪装成写入）
    let n1 = rows_of(&store, "hd-fault-a").len();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let n2 = rows_of(&store, "hd-fault-a").len();
    assert_eq!(n1, n2, "故障期间不得有新行（也没有部分写）");
    // B 全程运行且干净停止（隔离性）
    assert!(
        mgr.is_running("hd-fault-b"),
        "data-only endpoint 不得被拖死"
    );
    // 恢复：关故障 → A 行数重新增长（新 epoch，各自语义不变）
    faults.fail_sticky.store(false, Ordering::Relaxed);
    faults.fail_commit_at.store(0, Ordering::Relaxed);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if rows_of(&store, "hd-fault-a").len() > n2 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "恢复后 60s 无新行");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(mgr.is_running("hd-fault-b"));
    assert_eq!(mgr.stop_endpoint("hd-fault-b").await, Ok(true));
    src_a.stop().await;
    let _ = std::fs::remove_file(&db);
}
