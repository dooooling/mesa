//! NCK standalone 回环 emulator（ADR 0002 Layer4）。
//!
//! 把 `sinumerik-nck` 的确定性回环脚手架（`NckFixture`）装成独立 TCP 服务：
//! 场景可配（happy / partial_bad / wrong_transport / short_payload /
//! malformed / disconnect_after_n / pdu_*），供 Mesa 自身与 Sharp7 等独立
//! 客户端对打同一个对端（differential 前置条件）。
//!
//! 非目标：协议正确性判定（那是 Wireshark/文档/真机的事）；生产用途
//! （无任何安全机制，仅回环测试）。
//!
//! 能力边界（冻结，勿超声明）：
//! ```text
//! 独立进程边界 / TCP-COTP 生命周期 / PDU 协商 / 故障注入 / reconnect ✅
//! NCK codec 独立正确性 / area-unit / module-column 正确性           ❌
//! ```
//! 本服务与被测驱动共享同一 `NckFixture` 实现：通过只能证明进程与传输层，
//! 不能证明 codec；codec 正确性由 ADR 0002 三源比对 + Sharp7 互操作（待接）承担。
//!
//! 用法：
//! ```text
//! mesa-nck-emulator --port 1102 --scenario happy --element-size 8
//! mesa-nck-emulator --port 1102 --scenario partial_bad --fail-items 0,2
//! mesa-nck-emulator --port 1102 --scenario disconnect_after_n --disconnect-after 5
//! mesa-nck-emulator --port 1102 --scenario pdu_240
//! ```
//!
//! 索引约定（冻结）：`--fail-items` 为 **0-based**（第 1 项写 `0`），
//! 与脚手架 `enumerate()` 口径一致，不做 1-based 转换（两套约定并存必错）。
//! `--disconnect-after` 为正整数（`0` 拒绝：零次即断开无意义，
//! 且与“成功 N 次后断开”语义冲突）。

use std::collections::HashSet;
use std::net::SocketAddr;

use mesa_driver_sinumerik_nck::{NckFixture, NckFixtureState};

/// READY 行前缀（冻结；E2E 只认此前缀行解析实际地址）。
pub const READY_PREFIX: &str = "MESA_NCK_EMULATOR_READY=";

/// 内置场景（CLI `--scenario` 取值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    /// 全 GOOD（序号 pattern 数据）。
    Happy,
    /// 指定项返回 `0x05`（`--fail-items 0,2`，0-based）。
    PartialBad,
    /// transport 改 `0x07`（P1-4 校验对象）。
    WrongTransport,
    /// 数据减半（P1-4 校验对象）。
    ShortPayload,
    /// 截断包（畸形）。
    Malformed,
    /// 服务 N 次读后主动断开（`--disconnect-after N`）。
    DisconnectAfterN,
    /// 协商 PDU 上限档（240/480/960）。
    Pdu(u16),
}

impl Scenario {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "happy" => Ok(Scenario::Happy),
            "partial_bad" => Ok(Scenario::PartialBad),
            "wrong_transport" => Ok(Scenario::WrongTransport),
            "short_payload" => Ok(Scenario::ShortPayload),
            "malformed" => Ok(Scenario::Malformed),
            "disconnect_after_n" => Ok(Scenario::DisconnectAfterN),
            "pdu_240" => Ok(Scenario::Pdu(240)),
            "pdu_480" => Ok(Scenario::Pdu(480)),
            "pdu_960" => Ok(Scenario::Pdu(960)),
            _ => Err(format!(
                "未知 scenario `{s}`（happy/partial_bad/wrong_transport/short_payload/malformed/disconnect_after_n/pdu_240/pdu_480/pdu_960）"
            )),
        }
    }
}

/// CLI 解析结果（纯数据；`state_for` 为纯函数，可单测）。
#[derive(Debug, Clone)]
struct Cli {
    addr: SocketAddr,
    scenario: Scenario,
    element_size: usize,
    fail_items: HashSet<usize>,
    disconnect_after: Option<usize>,
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut port: u16 = 1102;
    let mut scenario = Scenario::Happy;
    let mut element_size: usize = 8;
    let mut fail_items = HashSet::new();
    let mut disconnect_after: Option<usize> = None;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => {
                port = it
                    .next()
                    .ok_or("--port 缺值")?
                    .parse()
                    .map_err(|_| "--port 需为 u16")?
            }
            "--scenario" => {
                scenario = Scenario::parse(it.next().ok_or("--scenario 缺值")?)?;
            }
            "--element-size" => {
                element_size = it
                    .next()
                    .ok_or("--element-size 缺值")?
                    .parse()
                    .map_err(|_| "--element-size 需为正整数")?;
                if element_size == 0 {
                    return Err("--element-size 需为正整数".into());
                }
            }
            "--fail-items" => {
                let s = it.next().ok_or("--fail-items 缺值")?;
                for part in s.split(',') {
                    fail_items.insert(
                        part.trim()
                            .parse()
                            .map_err(|_| "--fail-items 需为逗号分隔索引")?,
                    );
                }
            }
            "--disconnect-after" => {
                let n: usize = it
                    .next()
                    .ok_or("--disconnect-after 缺值")?
                    .parse()
                    .map_err(|_| "--disconnect-after 需为正整数")?;
                // fail-closed：0 无意义（“零次即断开”与场景语义冲突），直接拒绝。
                if n == 0 {
                    return Err("--disconnect-after 需为正整数（0 无意义，已拒绝）".into());
                }
                disconnect_after = Some(n);
            }
            _ => return Err(format!("未知参数 `{a}`")),
        }
    }
    Ok(Cli {
        addr: SocketAddr::from(([127, 0, 0, 1], port)),
        scenario,
        element_size,
        fail_items,
        disconnect_after,
    })
}

/// CLI → 脚手架状态（纯函数；场景语义唯一落点）。
fn state_for(cli: &Cli) -> Result<NckFixtureState, String> {
    let mut st = NckFixtureState {
        element_size: cli.element_size,
        ..Default::default()
    };
    match cli.scenario {
        Scenario::Happy => {}
        Scenario::PartialBad => {
            if cli.fail_items.is_empty() {
                return Err("partial_bad 需 --fail-items 指定 BAD 项索引".into());
            }
            st.fail_items = cli.fail_items.clone();
        }
        Scenario::WrongTransport => st.wrong_transport = true,
        Scenario::ShortPayload => st.short_payload = true,
        Scenario::Malformed => st.truncate = true,
        Scenario::DisconnectAfterN => {
            st.disconnect_after_reads = Some(
                cli.disconnect_after
                    .ok_or("--disconnect-after 缺值（disconnect_after_n 场景必填）")?,
            );
        }
        Scenario::Pdu(pdu) => st.max_pdu = pdu,
    }
    Ok(st)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = parse_args(&args).unwrap_or_else(|e| {
        eprintln!("参数错误: {e}");
        std::process::exit(2);
    });
    let state = state_for(&cli).unwrap_or_else(|e| {
        eprintln!("场景错误: {e}");
        std::process::exit(2);
    });
    let fx = NckFixture::spawn_with_state(cli.addr, state).await;
    eprintln!("nck-emulator listening on {}", fx.addr);
    // READY 协议：`--port 0` 时端口由 OS 在 bind 瞬间原子分配——测试禁止
    // 预占端口（free_port→release→bind 是 TOCTOU，main CI #120 实证），
    // 只认子进程自报的本行（人类可读行保留，机器只解析前缀）。
    eprintln!("{READY_PREFIX}{}", fx.addr);
    // 前台运行：Ctrl-C 即退出（测试工具，无需优雅关闭）。
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn defaults_are_happy_path() {
        let cli = parse_args(&args(&[])).unwrap();
        assert_eq!(cli.addr.port(), 1102);
        assert_eq!(cli.scenario, Scenario::Happy);
        let st = state_for(&cli).unwrap();
        assert_eq!(st.element_size, 8);
        assert!(st.fail_items.is_empty());
    }

    #[test]
    fn each_scenario_maps_state() {
        let cli = parse_args(&args(&["--scenario", "partial_bad", "--fail-items", "1,3"])).unwrap();
        let st = state_for(&cli).unwrap();
        assert!(st.fail_items.contains(&1) && st.fail_items.contains(&3));

        let cli = parse_args(&args(&["--scenario", "wrong_transport"])).unwrap();
        assert!(state_for(&cli).unwrap().wrong_transport);

        let cli = parse_args(&args(&["--scenario", "short_payload"])).unwrap();
        assert!(state_for(&cli).unwrap().short_payload);

        let cli = parse_args(&args(&["--scenario", "malformed"])).unwrap();
        assert!(state_for(&cli).unwrap().truncate);

        let cli = parse_args(&args(&[
            "--scenario",
            "disconnect_after_n",
            "--disconnect-after",
            "5",
        ]))
        .unwrap();
        assert_eq!(state_for(&cli).unwrap().disconnect_after_reads, Some(5));

        for (name, pdu) in [("pdu_240", 240), ("pdu_480", 480), ("pdu_960", 960)] {
            let cli = parse_args(&args(&["--scenario", name])).unwrap();
            assert_eq!(state_for(&cli).unwrap().max_pdu, pdu);
        }
    }

    #[test]
    fn evidence_lock_is_machine_readable() {
        // 外部源锁定文件（ADR 0002）：字段缺失/非法即证据失效，必须 loud。
        // 路径以本 crate 为锚（tools/nck-emulator → ../../docs），搬仓即显式失败。
        let path = format!(
            "{}/../../docs/evidence/nck-external-sources.lock.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).expect("证据锁文件缺失");
        let v: serde_json::Value = serde_json::from_str(&text).expect("锁文件非法 JSON");
        let sources = v
            .get("sources")
            .and_then(|x| x.as_array())
            .expect("锁文件缺 sources 数组");
        assert!(sources.len() >= 3, "至少锁定三源，实际 {}", sources.len());
        for s in sources {
            for key in ["name", "repo", "commit", "files", "role", "covers"] {
                assert!(s.get(key).is_some(), "源缺字段 `{key}`");
            }
            let commit = s.get("commit").and_then(|x| x.as_str()).unwrap_or("");
            assert_eq!(commit.len(), 40, "commit 必须为完整 40 位 SHA");
            assert!(
                commit.chars().all(|c| c.is_ascii_hexdigit()),
                "commit 非 hex"
            );
            assert!(
                !s.get("covers")
                    .and_then(|x| x.as_array())
                    .map(|a| a.is_empty())
                    .unwrap_or(true),
                "covers 不得为空"
            );
        }
    }

    #[test]
    fn bad_inputs_rejected() {
        assert!(parse_args(&args(&["--nope"])).is_err());
        assert!(parse_args(&args(&["--scenario", "nope"])).is_err());
        assert!(parse_args(&args(&["--port"])).is_err());
        assert!(parse_args(&args(&["--element-size", "0"])).is_err());
        // --disconnect-after 0 直接拒绝（不解释成“成功一次后断开”）。
        assert!(
            parse_args(&args(&[
                "--scenario",
                "disconnect_after_n",
                "--disconnect-after",
                "0"
            ]))
            .is_err()
        );
        // partial_bad 无索引、disconnect_after_n 无次数 → 场景错误。
        let cli = parse_args(&args(&["--scenario", "partial_bad"])).unwrap();
        assert!(state_for(&cli).is_err());
        let cli = parse_args(&args(&["--scenario", "disconnect_after_n"])).unwrap();
        assert!(state_for(&cli).is_err());
    }
}
