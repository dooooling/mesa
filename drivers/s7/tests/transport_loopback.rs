//! PR1 零回归证据：`s7` Driver 经 `mesa-s7-transport` 对回环假 PLC 的端到端读。
//!
//! 覆盖：建连握手/PDU 协商、单读、多项分片顺序、逐项 BAD 隔离、连续区批量、
//! SZL 诊断。断言的是搬迁前后不变的语义（基数/顺序/隔离），不是具体数值。

use mesa_driver_s7::client::{ReadItem, S7Client, S7ConnConfig};
use mesa_s7_transport::fixture::{FixtureState, spawn_fake_s7};

async fn connect_to(state: FixtureState) -> (S7Client, tokio::task::JoinHandle<()>) {
    let (addr, handle) = spawn_fake_s7(state).await;
    let cfg = S7ConnConfig {
        host: "127.0.0.1".into(),
        port: addr.port(),
        ..Default::default()
    };
    let client = S7Client::connect(cfg).await.expect("回环建连");
    (client, handle)
}

fn real(addr: &str) -> ReadItem {
    use mesa_driver_s7::{S7Kind, parse_address};
    ReadItem {
        addr: parse_address(addr).unwrap(),
        kind: S7Kind::Real,
    }
}

#[tokio::test]
async fn driver_single_and_multi_read_ordered() {
    let (mut c, h) = connect_to(FixtureState::default()).await;
    // PDU 协商：请求默认 480，脚手架默认上限 480 → 480。
    assert_eq!(c.pdu_length(), 480);
    let out = c.read_vars(&[real("DB10.DBD0")]).await.unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].as_ref().unwrap(), &vec![0x01; 4]);

    // 25 项 REAL：首项 16、后续 20，预算 448 → 19+6 两包，顺序必须连续。
    // NOTE: 同一连接序号连续，前一次单读已消耗序号 0，本批从序号 1 起。
    let items: Vec<_> = (0..25)
        .map(|i| real(&format!("DB10.DBD{}", i * 4)))
        .collect();
    let out = c.read_vars(&items).await.unwrap();
    assert_eq!(out.len(), 25);
    for (k, raw) in out.iter().enumerate() {
        assert_eq!(raw.as_ref().unwrap(), &vec![(k as u8) + 2; 4], "第 {k} 项");
    }
    h.abort();
}

#[tokio::test]
async fn driver_partial_bad_isolated_then_bulk_and_szl() {
    let mut st = FixtureState::default();
    st.fail_items.insert(1);
    let (mut c, h) = connect_to(st).await;
    let items: Vec<_> = (0..3)
        .map(|i| real(&format!("DB10.DBD{}", i * 4)))
        .collect();
    let out = c.read_vars(&items).await.unwrap();
    assert_eq!(out.len(), 3);
    assert!(out[0].is_some());
    assert!(out[1].is_none(), "第 1 项 BAD 必须隔离为 None");
    assert_eq!(out[2].as_ref().unwrap(), &vec![0x03; 4]);

    // 连续区批量：DB10 起 8 字节一次返回。
    use mesa_driver_s7::parse_address;
    let ranges = vec![(parse_address("DB10.DBD0").unwrap(), 8)];
    let bulk = c.read_byte_ranges(&ranges).await.unwrap();
    assert_eq!(bulk.len(), 1);
    assert_eq!(bulk[0].as_ref().unwrap().len(), 8);

    // SZL 诊断透传。
    let payload = c.read_szl(0x0011, 1).await.unwrap();
    assert!(!payload.is_empty());
    h.abort();
}
