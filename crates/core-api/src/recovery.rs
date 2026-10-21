//! 开机自动恢复的**产品策略**（不是通用生命周期语义）。
//!
//! 与 `config-store` 的 `RecoveryPlan`（围栏 / 正向凭证 / DB 可写性）分层：
//! 那一层回答"**能不能证明**这是用户意图"；本模块回答"**即使能证明**，这个
//! 驱动也不允许在开机时自动拉起"。
//!
//! 828D 一期约束（产品决策，评审已接受）：
//! - `sinumerik-nck` 端点**开机一律不自动恢复**，无论凭证、围栏、
//!   `desired_running` 为何；
//! - 必须由现场显式 `POST /start` 才启动；
//! - 其他驱动行为不变（后续是否单独收紧由独立决策确定）。
//!
//! 为什么放在 Core 而不是 Driver：开机恢复是 Core 的编排行为；Driver 只应
//! 描述"我是什么、怎么连"，不应持有"Core 该不该在启动时拉起我"的产品策略。
//! 这也保证策略只有一个判定位点；REST Start / Bootstrap 属于**显式**启动，
//! 不受本策略影响（本策略只约束开机恢复循环）。
//!
//! 迁移语义（升级路径，评审已接受）：
//! - 升级前已在运行的端点（DB `desired_running=true`）没有
//!   `<db>.intent-proof.json` 正向凭证 ⇒ 升级后**首次重启不会自动恢复**；
//! - 现场需在重启后对每个应运行的端点显式 `POST /start` 一次；成功后门会
//!   写入正向凭证并清除围栏，后续重启恢复正常自动恢复（NCK 除外，一期永不
//!   自动恢复）。
//!
//! 现场通知要点（一句话）：**升级后首次重启不会自动恢复升级前在运行的端点，
//! 请重启后逐台显式 `POST /start` 确认；828D/NCK 端点任何重启都必须显式启动。**

/// SINUMERIK NCK 驱动的 `driver_id`（ADR 0001）。
pub const NCK_DRIVER_ID: &str = "sinumerik-nck";

/// 该驱动是否允许在 Mesad 开机时自动恢复。
///
/// 只做**产品策略**判定；围栏 / 正向凭证 / DB 可写性仍由
/// `ConfigStore::recovery_plan()` 在调用方逐端点判定。
pub fn boot_auto_restore_allowed(driver_id: &str) -> bool {
    driver_id != NCK_DRIVER_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P0-C 的产品约束：NCK 一律不允许自动恢复（与 desired/围栏/凭证无关，
    /// 本函数根本看不到这些输入）。
    #[test]
    fn nck_is_never_auto_restored() {
        assert!(!boot_auto_restore_allowed(NCK_DRIVER_ID));
        assert!(!boot_auto_restore_allowed("sinumerik-nck"));
    }

    /// 对照组：其他驱动行为不变（仍交给既有围栏/凭证判据）。
    #[test]
    fn other_drivers_keep_existing_recovery_behavior() {
        for id in ["test-driver", "s7", "focas2", "opcua"] {
            assert!(
                boot_auto_restore_allowed(id),
                "非 NCK 驱动不得被本产品策略拦截: {id}"
            );
        }
        // 只做精确匹配：名字里含 nck 但不是该驱动的 id 不受影响。
        assert!(boot_auto_restore_allowed("sinumerik-nck-sim"));
        assert!(boot_auto_restore_allowed("SINUMERIK-NCK"));
    }
}
