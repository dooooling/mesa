//! 通过实际 DLL 差分的冻结帧回放到生产适配器；CI 无需 DLL 或 NCGuide。

use crate::address::{FocasAddress, parse_address};
use crate::focas_api::FocasApi;
use mesa_core_types::Value;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Deserialize)]
struct Case {
    address: String,
    expected: Value,
}

#[tokio::test]
async fn ready_dll_evidence_replays_final_values() {
    for profile in ["baseline", "minimum", "maximum"] {
        let dir = super::fixture_dir()
            .join("dll_ready_synthetic")
            .join(profile);
        let index: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("index.json")).unwrap()).unwrap();
        assert_eq!(index["kind"], "synthetic-derived");
        assert_eq!(index["hardware_acceptance"], false);
        for (name, hash) in index["files"].as_object().unwrap() {
            assert_eq!(
                format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(dir.join(name)).unwrap())
                ),
                hash.as_str().unwrap()
            );
        }
        let cases: Vec<Case> =
            serde_json::from_slice(&std::fs::read(dir.join("cases.json")).unwrap()).unwrap();
        let replay: Vec<(Vec<u8>, Vec<u8>)> = index["replay"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    std::fs::read(dir.join(r["request"].as_str().unwrap())).unwrap(),
                    std::fs::read(dir.join(r["response"].as_str().unwrap())).unwrap(),
                )
            })
            .collect();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for (request, response) in replay {
                let mut got = vec![0; request.len()];
                stream.read_exact(&mut got).await.unwrap();
                assert_eq!(got, request, "生产请求偏离已经 DLL 对照的固定序列");
                // 帧头与正文分开发送，避免测试仅覆盖整帧单次到达的情况。
                stream.write_all(&response[..7]).await.unwrap();
                stream.write_all(&response[7..]).await.unwrap();
            }
        });
        let api = super::WireFocasApi::new(Duration::from_secs(3));
        api.connect("127.0.0.1", port, 3000).await.unwrap();
        for c in cases {
            let a = match c.address.as_str() {
                "active_spindle" => FocasAddress::ActiveSpindleSpeed,
                "diagnosis301axis3" => FocasAddress::Diagnosis {
                    number: 301,
                    axis: 3,
                },
                _ => parse_address(&c.address).unwrap(),
            };
            assert_eq!(
                api.read_batch(&[a]).await.unwrap(),
                vec![c.expected],
                "{profile}/{}",
                c.address
            );
        }
        api.disconnect().await;
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}
