//! NCK 拓扑类型（ADR 0001 §10：`NckTopology`）。
//!
//! Siemens 原生组织：Mode Group（B）→ Channel（C）→ Axis/Spindle（A），
//! Tool（T）/Drive（V/H）独立。拓扑回答“设备上有什么实例”（通道数、轴名），
//! Catalog 回答“变量长什么样”（类型、线缆映射）；browse 把两者拼成虚拟树。
//!
//! V1 状态：类型与显式构造就位，真机实例数据待 probe anchor 回填（见
//! `probe` 的 `NCK_ANCHOR_PENDING`）。构造器只接受显式参数，不猜默认值
//! （通道/轴数无从推导，缺省即错）。

use std::collections::HashMap;

/// 单轴（含主轴；Siemens 轴名如 X/Y/Z/SP1，由设备上报，原样保留）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NckAxis {
    /// 轴序号（Area A 的 area_no 候选，待真机确认映射）。
    pub index: u16,
    pub name: String,
}

/// 单通道（Area C 的 area_no 候选，待真机确认映射）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NckChannel {
    pub number: u16,
    pub name: String,
    pub axes: Vec<NckAxis>,
}

/// NCK 拓扑快照（probe 产物；browse 输入）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NckTopology {
    pub channels: Vec<NckChannel>,
    /// 通道外轴（如独立主轴），key 为上报名。
    pub extra_axes: Vec<NckAxis>,
}

impl NckTopology {
    /// 空拓扑（probe 失败/anchor 缺失时 browse 退化为纯 Catalog 树）。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 显式构造（数量与名称全部由调用方给出，不做任何推导）。
    pub fn from_channels(channels: Vec<NckChannel>) -> Self {
        Self {
            channels,
            extra_axes: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty() && self.extra_axes.is_empty()
    }

    /// 通道号 → 通道（browse 实例展开用）。
    pub fn channel(&self, number: u16) -> Option<&NckChannel> {
        self.channels.iter().find(|c| c.number == number)
    }

    /// 全轴索引（browse 校验 line 合法性用；真机映射确认前仅作存在性检查）。
    pub fn axis_names(&self) -> HashMap<u16, &str> {
        let mut m = HashMap::new();
        for c in &self.channels {
            for a in &c.axes {
                m.entry(a.index).or_insert(a.name.as_str());
            }
        }
        for a in &self.extra_axes {
            m.entry(a.index).or_insert(a.name.as_str());
        }
        m
    }
}

/// 拓扑节点路径 id（browse parent/child 的稳定身份，与变量 canonical key
/// 同一 `nck://` 命名空间，互不碰撞：实例路径段数为 2，变量键段数为 4）。
pub fn channel_path(number: u16) -> String {
    format!("nck://C/{number}")
}

pub fn axis_path(index: u16) -> String {
    format!("nck://A/{index}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_paths_and_lookup() {
        let topo = NckTopology::from_channels(vec![
            NckChannel {
                number: 1,
                name: "CHAN1".into(),
                axes: vec![
                    NckAxis {
                        index: 1,
                        name: "X".into(),
                    },
                    NckAxis {
                        index: 2,
                        name: "SP1".into(),
                    },
                ],
            },
            NckChannel {
                number: 2,
                name: "CHAN2".into(),
                axes: vec![],
            },
        ]);
        assert!(!topo.is_empty());
        assert_eq!(topo.channel(1).unwrap().name, "CHAN1");
        assert!(topo.channel(9).is_none());
        assert_eq!(channel_path(1), "nck://C/1");
        assert_eq!(axis_path(3), "nck://A/3");
        let names = topo.axis_names();
        assert_eq!(names[&1], "X");
        assert_eq!(names[&2], "SP1");
        assert!(NckTopology::empty().is_empty());
    }
}
