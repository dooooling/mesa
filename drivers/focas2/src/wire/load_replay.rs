//! 负载实际 DLL 差分证据回放；不依赖 Windows DLL，CI 可覆盖正式采集路径。

use crate::address::{FocasAddress, SpindleKind};
use crate::focas_api::FocasApi;
use mesa_core_types::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn addresses() -> Vec<FocasAddress> {
    let mut result: Vec<_> = (1..=4)
        .map(|spindle| FocasAddress::Spindle {
            spindle,
            kind: SpindleKind::Load,
        })
        .collect();
    result.extend((1..=4).map(|axis| FocasAddress::ServoLoad { axis }));
    result.extend([
        result[0].clone(),
        result[4].clone(),
        FocasAddress::ServoLoad { axis: 0 },
        FocasAddress::Spindle {
            spindle: 5,
            kind: SpindleKind::Load,
        },
    ]);
    result
}

async fn server(script: Vec<(Vec<u8>, Vec<u8>)>) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        for (request, response) in script {
            let mut got = vec![0; request.len()];
            socket.read_exact(&mut got).await.unwrap();
            assert_eq!(got, request, "正式负载请求偏离实际 DLL 对照的序列");
            socket.write_all(&response[..7]).await.unwrap();
            socket.write_all(&response[7..]).await.unwrap();
        }
    });
    (port, worker)
}

fn reply_payload(slots: &[super::frame::ReplySubpacket]) -> Vec<u8> {
    use super::frame::{GenericSubpacket, encode_generic_request};
    encode_generic_request(
        &slots
            .iter()
            .map(|s| {
                let mut payload = Vec::new();
                for short in [s.status, s.detail1, s.detail2, s.data.len() as i16] {
                    payload.extend_from_slice(&short.to_be_bytes());
                }
                payload.extend_from_slice(&s.data);
                GenericSubpacket {
                    control_device: s.device,
                    function: ((s.path as u32) << 16) | s.command as u32,
                    payload,
                }
            })
            .collect::<Vec<_>>(),
    )
}

#[tokio::test]
async fn load_dll_evidence_replays_production_batches() {
    let root = super::fixture_dir().join("load_production_dll_v2");
    let index = json(&root.join("index.json"));
    assert_eq!(index["kind"], "synthetic-derived");
    let cases = index["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 11, "负载分支/边界样本不得静默缩减");
    for case in cases {
        let dir = root.join(case["name"].as_str().unwrap());
        for (name, expected) in case["files"].as_object().unwrap() {
            let bytes = std::fs::read(dir.join(name)).unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(bytes)),
                expected.as_str().unwrap()
            );
        }
        let traffic = json(&dir.join("server.json"));
        let rows = traffic["records"].as_array().unwrap();
        // 原始日志包含 DLL 两连接；仅回放最后建立的 Wire 连接，不能混用流。
        let peer = &rows.iter().rev().find(|r| r["tag"] == "open2").unwrap()["peer"];
        let script = rows
            .iter()
            .filter(|r| &r["peer"] == peer)
            .map(|r| {
                (
                    std::fs::read(dir.join("traffic").join(r["request"].as_str().unwrap()))
                        .unwrap(),
                    std::fs::read(dir.join("traffic").join(r["response"].as_str().unwrap()))
                        .unwrap(),
                )
            })
            .collect();
        let (port, worker) = server(script).await;
        let api = super::WireFocasApi::new(Duration::from_secs(3));
        api.connect("127.0.0.1", port, 3000).await.unwrap();
        let comparisons = json(&dir.join("samples/production-comparison.json"));
        for batch in 0..2 {
            let expected: Vec<Value> = comparisons
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["batch"] == batch)
                .map(|r| serde_json::from_value(r["actual"].clone()).unwrap())
                .collect();
            assert_eq!(
                api.read_batch(&addresses()).await.unwrap(),
                expected,
                "{} batch{batch}",
                dir.display()
            );
        }
        api.disconnect().await;
        tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap();
    }
}

/// 合法远端错误应保留会话供下一次读取；坏长度必须使会话失效。
/// 错误覆盖各槽，防止只检查首槽而漏掉名称、数值或尾部错误。
#[tokio::test]
async fn load_remote_errors_and_malformed_session_lifecycle() {
    use super::frame::{FocasFrame, assemble, decode_header, decode_reply_payload};
    use super::wire::DEV_CNC;
    let root = super::fixture_dir();
    let open = std::fs::read(root.join("load_synthetic_handshake/open_response.bin")).unwrap();
    for servo in [false, true] {
        let folder =
            root.join("load_pair_165_20261010")
                .join(if servo { "servo" } else { "type0" });
        let request = std::fs::read(folder.join("native_request.bin")).unwrap();
        let response = std::fs::read(folder.join("native_response.bin")).unwrap();
        let original = assemble(
            decode_header(&response[..10].try_into().unwrap()).unwrap(),
            response[10..].to_vec(),
        )
        .unwrap();
        let address = if servo {
            FocasAddress::ServoLoad { axis: 1 }
        } else {
            FocasAddress::Spindle {
                spindle: 1,
                kind: SpindleKind::Load,
            }
        };
        for slot in 0..4 {
            let mut slots = decode_reply_payload(&original.payload).unwrap();
            slots[slot].status = -3;
            slots[slot].detail1 = 7;
            slots[slot].detail2 = 8;
            let error = FocasFrame {
                payload: reply_payload(&slots),
                ..original.clone()
            }
            .encode();
            let (port, worker) = server(vec![
                (FocasFrame::open_request().encode(), open.clone()),
                (request.clone(), error),
                (request.clone(), response.clone()),
                (
                    FocasFrame::close_request().encode(),
                    vec![0xa0, 0xa0, 0xa0, 0xa0, 0, 3, 2, 2, 0, 0],
                ),
            ])
            .await;
            let api = super::WireFocasApi::new(Duration::from_secs(3));
            api.connect("127.0.0.1", port, 3000).await.unwrap();
            assert!(
                matches!(&api.read_batch(std::slice::from_ref(&address)).await.unwrap()[0], Value::String(s) if s.starts_with("ERR:") && s.contains("-3"))
            );
            assert!(matches!(
                &api.read_batch(std::slice::from_ref(&address))
                    .await
                    .unwrap()[0],
                Value::F64(_)
            ));
            api.disconnect().await;
            worker.await.unwrap();
        }
        let mut slots = decode_reply_payload(&original.payload).unwrap();
        assert_eq!(slots[2].device, DEV_CNC);
        slots[2].data = vec![];
        let broken = FocasFrame {
            payload: reply_payload(&slots),
            ..original
        }
        .encode();
        let (port, worker) = server(vec![
            (FocasFrame::open_request().encode(), open.clone()),
            (request, broken),
        ])
        .await;
        let api = super::WireFocasApi::new(Duration::from_secs(3));
        api.connect("127.0.0.1", port, 3000).await.unwrap();
        assert!(
            api.read_batch(std::slice::from_ref(&address))
                .await
                .is_err()
        );
        assert!(
            api.read_batch(&[address]).await.is_err(),
            "坏帧后不得复用会话"
        );
        api.disconnect().await;
        worker.await.unwrap();
    }
}
