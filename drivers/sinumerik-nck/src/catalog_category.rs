//! 目录**导航分类**：把每条定义投影到前端建树用的 5 个分类码（Driver 侧临时规则）。
//!
//! ## 硬边界：这是导航规则，**不是**手册分类
//!
//! 828D 的四份手册（NC 变量清单 / 系统变量 / 机床数据 / S120 参数手册）都**不给**
//! 「这条变量属于哪一类」的字段，而 `r1.json` 也没有 `category` 且已冻结。因此本
//! 模块产出的分类只有一种性质：**驱动侧为了建树而临时制定的导航规则**——它只回答
//! 「用户能沿哪条路径找到它」，不回答「手册说它是什么」。
//!
//! 由此产生的三条纪律（评审明确要求，改动本模块前先读）：
//!
//! 1. 输出必须自带 `category_basis`
//!    （[`CATEGORY_BASIS_NAVIGATION_RULE`] = `"navigation_rule"`），让消费方一眼
//!    看出这不是定义事实；方案 B 落地后才允许变成 `"definition"`。
//! 2. 注释、文档、日志**禁止**把它写成「手册分类」「已确认分类」「权威分类」。
//! 3. 分类不参与 `digest`、不进 `DIGESTS.lock`、不改 `VariableDefinition` 的字段：
//!    `r1.json` 一字节不动，本表纯属**投影期**计算。
//!
//! ## 为什么是显式表而不是前缀猜测
//!
//! 前缀/ID 形状与分类并非一一对应：`channel.r_parameter` 是 `channel.` 前缀却属
//! 「系统变量」（R 参数＝系统变量 Rn），而 `channel.machine_axis_assignment` 也是
//! `channel.` 前缀却是机床数据（MD20070）。任何「按前缀散落各处的猜测」都会在这
//! 两处失手，而且失手时**静默**给出错误分类。故分类一律写成 [`CATEGORY_RULES`]
//! **逐条显式表**（42 条对 42 条），未命中的 ID **不猜**：落
//! [`CatalogCategory::Unclassified`]。
//!
//! ## 证据锚点：表里每条规则都必须可被定义自身证伪
//!
//! 每条规则除分类外还带一个 [`EvidenceAnchor`]：分类**只有在定义里能找到对应证据
//! 时才生效**，否则同样落 `unclassified`。这让「表写错了」和「`r1.json` 换了」都会
//! **失败关闭**，而不是继续输出一个看似合理的分类。锚点只用定义里已有的字段
//! （`source[]` / `golden_ref[]` / `addressing.column`），不额外引入真值。
//!
//! ## 后续（方案 B）
//!
//! TODO(方案 B)：正式分类走定义修订（`revision` + `DIGESTS.lock`）建立权威字段；
//! 届时 `category_basis` 应变为 `"definition"`，本表的角色退化为「历史对照」，
//! 不得再被当作分类来源。

use crate::definition::{SourceKind, VariableDefinition};
use crate::targets::DriveEntry;

/// 分类依据：**导航规则**（本轮唯一取值）。
///
/// 含义：`category` 是驱动侧临时导航规则算出来的，**不是**手册确认的事实。
/// 方案 B（正式定义修订）落地后，该值应改为 `"definition"`。
pub const CATEGORY_BASIS_NAVIGATION_RULE: &str = "navigation_rule";

/// 前端建树用的 5 个分类码（词表与前端导航树一一对应；**不新增第六个**）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CatalogCategory {
    /// CNC 变量：NC 变量清单（OPI 入口）里的通道/轴/主轴/方式组运行数据。
    CncVariable,
    /// 系统变量：用户可编程的系统变量（本轮只有 R 参数一类）。
    SystemVariable,
    /// 机床数据：按 MD 编号定位的 NC 机床数据。
    MachineData,
    /// SINAMICS 驱动参数：驱动侧 `r*/p*` 参数。
    DriveParameter,
    /// 其他 / 待分类：无法从定义内证据可靠归类的条目。**这是唯一允许「不知道」的去处**。
    Unclassified,
}

impl CatalogCategory {
    /// 对外词表（JSON 取值，小写下划线；冻结后不得改名）。
    pub const fn as_str(self) -> &'static str {
        match self {
            CatalogCategory::CncVariable => "cnc_variable",
            CatalogCategory::SystemVariable => "system_variable",
            CatalogCategory::MachineData => "machine_data",
            CatalogCategory::DriveParameter => "drive_parameter",
            CatalogCategory::Unclassified => "unclassified",
        }
    }

    /// 全部合法取值（测试与消费方共用；顺序即导航树顺序）。
    pub const ALL: [CatalogCategory; 5] = [
        CatalogCategory::CncVariable,
        CatalogCategory::SystemVariable,
        CatalogCategory::MachineData,
        CatalogCategory::DriveParameter,
        CatalogCategory::Unclassified,
    ];
}

/// 分类所依赖的**定义内证据锚点**（命中才采用表里的分类）。
///
/// 设计意图：把「凭什么这么归」写成可执行判据，而不是只写在注释里。锚点全部取自
/// 定义既有字段，因此不会引入第二份真值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceAnchor {
    /// `source[]` 里必须有一条 `kind=manual` 且 `reference` 含该子串。
    ///
    /// 用来表达「这条定义引的是哪个手册族」：`"NC variables"` 表示 828D NC 变量
    /// 清单，`"SINAMICS"` 表示 S120/S150 参数手册。
    ManualReference(&'static str),
    /// `golden_ref[]` 里必须有一条入口是驱动对象（`drive_object`，module 130）。
    ///
    /// 用来表达「这条定义确实走驱动对象入口」，而不是仅凭 ID 里有 `drive` 就归
    /// 驱动参数。
    DriveObjectEntry,
    /// `golden_ref[].variable` 必须逐字等于该符号（OPI 入口项的符号）。
    GoldenSymbol(&'static str),
    /// 机床数据双证据：符号逐字相等 **且** `addressing.column` 等于该 MD 编号。
    ///
    /// 机床数据手册按 MD 编号排序、定义按编号定位（列号即 MD 号），因此「符号 +
    /// 编号」同时命中才认为这条确实是机床数据；只凭名字里的 `MD` 前缀不足以定案
    /// （`AXCONF_MACHAX_USED` 就没有前缀）。
    MachineDataSymbol { symbol: &'static str, number: u16 },
}

/// 一条导航分类规则：`variable_id` → 分类 + 证据锚点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryRule {
    /// 定义 ID（逐字匹配，不支持前缀/通配）。
    pub variable_id: &'static str,
    /// 导航分类（**不含** `Unclassified`：待分类只能由失败关闭产生，见模块文档）。
    pub category: CatalogCategory,
    /// 该分类必须成立的定义内证据。
    pub anchor: EvidenceAnchor,
}

/// 分类结果：分类码 + 自证依据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryAssignment {
    pub category: CatalogCategory,
    /// 本轮恒为 [`CATEGORY_BASIS_NAVIGATION_RULE`]。
    pub basis: &'static str,
}

impl CategoryAssignment {
    /// 失败关闭的唯一出口：不确定就进「待分类」，绝不猜四类之一。
    const fn unclassified() -> Self {
        Self {
            category: CatalogCategory::Unclassified,
            basis: CATEGORY_BASIS_NAVIGATION_RULE,
        }
    }
}

/// 42 条定义的**显式**导航分类表（顺序沿用 `r1.json`，便于逐条对照审计）。
///
/// 分桶口径（导航层，非手册口径）：
/// - `cnc_variable`：NC 变量清单里的 OPI 入口项——寻址靠「块 + 列 + 行」，读的是
///   通道/轴/主轴/方式组的运行数据；
/// - `system_variable`：用户可编程的系统变量（本轮仅 R 参数，见下）；
/// - `machine_data`：按 MD 编号定位的机床数据（符号 + 编号双证据）；
/// - `drive_parameter`：SINAMICS 侧 `r*/p*` 参数（参数手册引用或驱动对象入口）。
///
/// 每条规则的 `//` 注释就是它的**一句话依据**（为什么是这一类、为什么不是相邻的
/// 那一类）。这些依据是导航理由，不是手册结论。
pub const CATEGORY_RULES: [CategoryRule; 42] = [
    // ---- CNC 变量：NC 变量清单（03/2025）的 C/B 区状态块 ----
    // channel.program.status：C/S `progStatus`（NC 变量手册 p.306），通道程序状态
    // 运行数据；寻址靠 C 区 + 块 S + 列 13，不是 MD 编号 ⇒ 不是机床数据；也不属于
    // 用户可编程的系统变量（无 $R/$A 类编程入口）。
    CategoryRule {
        variable_id: "channel.program.status",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // mode_group.op_mode：B/S `opMode`（NC 变量手册 p.89–90），方式组运行方式；
    // B 区是 NCK 的方式组数据区，仍属 NC 变量入口 ⇒ 不是机床数据（无 MD 编号）。
    CategoryRule {
        variable_id: "mode_group.op_mode",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // channel.program.name.main：C/SPARPP `progName`（p.288 附近），主程序名；
    // 通道运行数据 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "channel.program.name.main",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // channel.program.name.active：C/SPARP `progName`，当前活动程序名 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "channel.program.name.active",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // channel.program.line_number：C/SPARP `actLineNumber`，当前行号 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "channel.program.line_number",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // channel.program.block：C/SPARP `actBlock`，当前程序段 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "channel.program.block",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // axis.position.commanded_mcs：C/SGA `actToolBasePos`，指令位置；
    // 虽叫「轴位置」，但入口是 C 区 SGA 块（NC 变量清单），MD 手册里没有这条
    // 位置变量 ⇒ CNC 变量，不是机床数据。
    CategoryRule {
        variable_id: "axis.position.commanded_mcs",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // axis.position.actual_bcs：C/SEGA `actProgPosBKS`，实际位置 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "axis.position.actual_bcs",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // channel.feed.actual：C/S `actFeedRateIpo`，进给实际值。归 CNC 变量而不是
    // 「系统变量」的理由：它是 NC 变量清单里的**状态块项**（块 S、列固定），
    // 没有 $A_/$V_ 这类编程系统变量形态，用户也不是通过参数号找它。
    CategoryRule {
        variable_id: "channel.feed.actual",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // channel.feed.commanded：C/S `cmdFeedRateIpo`，进给指令值 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "channel.feed.commanded",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // channel.feed.override：C/S `feedRateIpoOvr`，进给倍率 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "channel.feed.override",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // spindle.speed.actual：C/SSP `actSpeed`，主轴实际转速 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "spindle.speed.actual",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // spindle.speed.commanded：C/SSP `cmdSpeed`，主轴指令转速 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "spindle.speed.commanded",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // spindle.speed.override：C/SSP `speedOvr`，主轴倍率 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "spindle.speed.override",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // channel.tool.number_active：C/S `actTNumber`（p.277），当前刀具号 ⇒ CNC 变量。
    CategoryRule {
        variable_id: "channel.tool.number_active",
        category: CatalogCategory::CncVariable,
        anchor: EvidenceAnchor::ManualReference("NC variables"),
    },
    // ---- 系统变量：本轮仅 R 参数 ----
    // channel.r_parameter：C/RP 块 `rpa`，即 R 参数。归「系统变量」的导航依据：
    // 828D 系统变量手册把 R 参数定义为「系统变量 Rn 或 R[n]（Real 类型的通道专用
    // 计算参数）」（见 `docs/reference/828d-manual-cross-reference.md` 的
    // sysvar 交叉引用行，p15 §3.3）；用户是按「我能编程的 R 号」找它，而不是按
    // 「通道状态块」找。
    // NOTE(导航判断而非事实)：同一条也出现在 NC 变量清单 5.3.15（RP: Arithmetic
    // parameters, p.219），两处都不构成「分类」。本轮按系统变量手册的措辞归类，
    // 属导航选择；方案 B 应以定义字段为准。
    CategoryRule {
        variable_id: "channel.r_parameter",
        category: CatalogCategory::SystemVariable,
        anchor: EvidenceAnchor::GoldenSymbol("rpa"),
    },
    // ---- SINAMICS 驱动参数：参数手册引用（19 条）----
    // drive.parameter.r0021：S120/S150 参数手册（11/2023）的 r0021，驱动侧参数
    // ⇒ 驱动参数。全部 19 条同构，逐条列出以保证「表与定义一一对应」。
    CategoryRule {
        variable_id: "drive.parameter.r0021",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0027：S120/S150 参数手册 r0027（仅参数身份）⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0027",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.motor.temperature：S120/S150 参数手册 r0035（含 p.71 无效值标记），
    // 电机温度 ⇒ 驱动参数（同一物理量的 NC 变量入口不存在，故与 CNC 变量无关）。
    CategoryRule {
        variable_id: "drive.motor.temperature",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0033：S120/S150 参数手册 r0033（候选语义）⇒ 驱动参数；
    // 语义未确认不影响**导航**归类（分类不依赖语义确认）。
    CategoryRule {
        variable_id: "drive.parameter.r0033",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0031：S120/S150 参数手册 r0031（候选语义）⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0031",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0062：S120/S150 参数手册 r0062（仅参数身份）⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0062",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0063：S120/S150 参数手册 r0063 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0063",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0064：S120/S150 参数手册 r0064 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0064",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0077：S120/S150 参数手册 r0077 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0077",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0078：S120/S150 参数手册 r0078 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0078",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0079：S120/S150 参数手册 r0079 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0079",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0080：S120/S150 参数手册 r0080 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0080",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0081：S120/S150 参数手册 r0081 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0081",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0082：S120/S150 参数手册 r0082 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0082",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0103：S120/S150 参数手册 r0103 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0103",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.p0105：S120/S150 参数手册 p0105（只读）⇒ 驱动参数；
    // `p` 前缀（可设参数）不改变导航归类。
    CategoryRule {
        variable_id: "drive.parameter.p0105",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0106：S120/S150 参数手册 r0106 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0106",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0107：S120/S150 参数手册 r0107 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0107",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // drive.parameter.r0108：S120/S150 参数手册 r0108 ⇒ 驱动参数。
    CategoryRule {
        variable_id: "drive.parameter.r0108",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::ManualReference("SINAMICS"),
    },
    // ---- 驱动参数：驱动对象入口（无 manual 来源的那一条）----
    // drive.object.name：`V/M/p0199` 的驱动对象名称。r1.json 里它的 source 只有
    // measurement + frozen_vector（**没有** manual 引用），因此锚点用「确实存在
    // drive_object 入口」而不是手册引用；p0199 的参数身份另见
    // `docs/reference/828d-definition-source-audit.md`（S120 参数手册 p136 交叉引用）。
    CategoryRule {
        variable_id: "drive.object.name",
        category: CatalogCategory::DriveParameter,
        anchor: EvidenceAnchor::DriveObjectEntry,
    },
    // ---- 机床数据：符号 + MD 编号双证据 ----
    // axis.name：`MDSA_AXCONF_MACHAX_NAME_TAB` = MD10000（机床轴名称表），
    // 列号即 MD 号 10000 ⇒ 机床数据；它不是 NC 变量清单里的状态块项。
    CategoryRule {
        variable_id: "axis.name",
        category: CatalogCategory::MachineData,
        anchor: EvidenceAnchor::MachineDataSymbol {
            symbol: "MDSA_AXCONF_MACHAX_NAME_TAB",
            number: 10000,
        },
    },
    // axis.ctrlout.module_number：`MDCA_CTRLOUT_MODULE_NR` = MD30110（设定值输出
    // 模块号）⇒ 机床数据。
    CategoryRule {
        variable_id: "axis.ctrlout.module_number",
        category: CatalogCategory::MachineData,
        anchor: EvidenceAnchor::MachineDataSymbol {
            symbol: "MDCA_CTRLOUT_MODULE_NR",
            number: 30110,
        },
    },
    // axis.encoder.module_number：`MDCA_ENC_MODULE_NR` = MD30220（实际值传送的
    // 驱动号/测量回路号）⇒ 机床数据。
    CategoryRule {
        variable_id: "axis.encoder.module_number",
        category: CatalogCategory::MachineData,
        anchor: EvidenceAnchor::MachineDataSymbol {
            symbol: "MDCA_ENC_MODULE_NR",
            number: 30220,
        },
    },
    // axis.ctrlout.type：`MDCA_CTRLOUT_TYPE` = MD30130（设定值输出类型）⇒ 机床数据。
    CategoryRule {
        variable_id: "axis.ctrlout.type",
        category: CatalogCategory::MachineData,
        anchor: EvidenceAnchor::MachineDataSymbol {
            symbol: "MDCA_CTRLOUT_TYPE",
            number: 30130,
        },
    },
    // channel.machine_axis_assignment：`AXCONF_MACHAX_USED` = MD20070（通道轴对应
    // 的机床轴）。**反例价值**：ID 前缀是 `channel.` 却属机床数据，证明「按前缀猜」
    // 会错；符号没有 `MD` 前缀，故用「符号 + 编号」双证据定案。
    CategoryRule {
        variable_id: "channel.machine_axis_assignment",
        category: CatalogCategory::MachineData,
        anchor: EvidenceAnchor::MachineDataSymbol {
            symbol: "AXCONF_MACHAX_USED",
            number: 20070,
        },
    },
    // axis.spindle_assignment：`SPIND_ASSIGN_TO_MACHAX` = MD35000（把机床轴定义
    // 为主轴）⇒ 机床数据。
    CategoryRule {
        variable_id: "axis.spindle_assignment",
        category: CatalogCategory::MachineData,
        anchor: EvidenceAnchor::MachineDataSymbol {
            symbol: "SPIND_ASSIGN_TO_MACHAX",
            number: 35000,
        },
    },
];

/// 按 ID 取规则（逐字匹配；找不到即无规则，调用方必须失败关闭）。
pub fn rule_for(variable_id: &str) -> Option<&'static CategoryRule> {
    CATEGORY_RULES
        .iter()
        .find(|rule| rule.variable_id == variable_id)
}

/// 证据锚点是否在定义里成立（成立才允许采用表里的分类）。
fn anchor_holds(anchor: EvidenceAnchor, def: &VariableDefinition) -> bool {
    match anchor {
        EvidenceAnchor::ManualReference(needle) => def
            .source
            .iter()
            .any(|s| s.kind == SourceKind::Manual && s.reference.contains(needle)),
        EvidenceAnchor::DriveObjectEntry => def
            .golden_ref
            .iter()
            .any(|g| g.entry == DriveEntry::DriveObject),
        EvidenceAnchor::GoldenSymbol(symbol) => def.golden_ref.iter().any(|g| g.variable == symbol),
        EvidenceAnchor::MachineDataSymbol { symbol, number } => {
            def.addressing.column == number && def.golden_ref.iter().any(|g| g.variable == symbol)
        }
    }
}

/// 一条定义的导航分类。
///
/// 失败关闭（**不许猜**）的三种情形，全部落 [`CatalogCategory::Unclassified`]：
/// 1. ID 不在 [`CATEGORY_RULES`] 内（新定义未登记，或 ID 拼错）；
/// 2. 有规则但**证据锚点不成立**（表与定义已漂移）；
/// 3. 将来的定义若把 `variable_id` 复用成别的对象，同样会因锚点失效而降级。
///
/// 返回值里的 `basis` 恒为 [`CATEGORY_BASIS_NAVIGATION_RULE`]：无论分类成败，
/// 本轮都不能声称这是手册事实。
pub fn classify_category(def: &VariableDefinition) -> CategoryAssignment {
    match rule_for(&def.variable_id) {
        Some(rule) if anchor_holds(rule.anchor, def) => CategoryAssignment {
            category: rule.category,
            basis: CATEGORY_BASIS_NAVIGATION_RULE,
        },
        Some(_) | None => CategoryAssignment::unclassified(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::DefinitionCatalog;
    use std::collections::BTreeSet;

    fn catalog() -> DefinitionCatalog {
        DefinitionCatalog::embedded().expect("内嵌定义目录必须通过装载期自检")
    }

    fn def(id: &str) -> VariableDefinition {
        catalog()
            .lookup(id, 1)
            .unwrap_or_else(|e| panic!("`{id}` 必须在内嵌目录里：{e}"))
            .clone()
    }

    /// 表与已发布定义**一一对应**：42 条全覆盖、无重复、无多余。
    #[test]
    fn rules_cover_every_published_definition_exactly_once() {
        let catalog = catalog();
        let ids: BTreeSet<&str> = catalog.iter().map(|d| d.variable_id.as_str()).collect();
        let rule_ids: BTreeSet<&str> = CATEGORY_RULES.iter().map(|r| r.variable_id).collect();
        assert_eq!(rule_ids.len(), CATEGORY_RULES.len(), "分类表不得有重复 ID");
        assert_eq!(
            rule_ids, ids,
            "分类表必须与已发布定义一一对应（新增定义必须显式登记，否则落 unclassified）"
        );
        assert_eq!(ids.len(), 42, "828d 已发布定义必须恰为 42 条");
    }

    /// 表里只允许「已决分类」；`unclassified` 只能由失败关闭产生，
    /// 否则「不知道」会被伪装成一条明确规则。
    #[test]
    fn table_never_declares_unclassified_directly() {
        assert!(
            CATEGORY_RULES
                .iter()
                .all(|r| r.category != CatalogCategory::Unclassified),
            "分类表不得直接声明 unclassified：待分类只能来自未命中/证据不足"
        );
    }

    /// 内嵌目录的 42 条**全部**命中锚点（否则说明表或定义已漂移）。
    #[test]
    fn every_embedded_definition_hits_its_anchor() {
        for d in catalog().iter() {
            let a = classify_category(d);
            assert_ne!(
                a.category,
                CatalogCategory::Unclassified,
                "`{}` 必须可归类：规则存在但证据锚点不成立（表与 r1.json 已漂移？）",
                d.variable_id
            );
            assert_eq!(a.basis, CATEGORY_BASIS_NAVIGATION_RULE);
        }
    }

    /// 反例 1：未登记的 ID **不得**被前缀/形状猜进四类之一。
    #[test]
    fn unknown_variable_id_falls_back_to_unclassified() {
        let mut d = def("channel.feed.actual");
        d.variable_id = "channel.feed.unknown".into();
        let a = classify_category(&d);
        assert_eq!(a.category, CatalogCategory::Unclassified);
        assert_eq!(a.basis, CATEGORY_BASIS_NAVIGATION_RULE);
    }

    /// 反例 2：ID 命中但证据锚点不成立 ⇒ 仍落 unclassified（不沿用表里的分类）。
    ///
    /// 构造方式：保留 `channel.feed.actual` 的 ID，但把手册来源去掉——此时定义自身
    /// 已不能证明「它引的是 NC 变量清单」，继续输出 `cnc_variable` 就等于猜。
    #[test]
    fn anchor_mismatch_falls_back_to_unclassified() {
        let mut d = def("channel.feed.actual");
        d.source.retain(|s| s.kind != SourceKind::Manual);
        assert_eq!(
            classify_category(&d).category,
            CatalogCategory::Unclassified,
            "手动来源被移除后不得仍归 CNC 变量"
        );

        // 机床数据的双证据：符号对、编号不对 ⇒ 同样不归类。
        let mut md = def("axis.ctrlout.module_number");
        md.addressing.column = 9999;
        assert_eq!(
            classify_category(&md).category,
            CatalogCategory::Unclassified,
            "MD 编号与符号不一致时不得仍归机床数据"
        );
    }

    /// 前缀不得决定分类：两个 `channel.` 前缀的定义分属不同分类。
    #[test]
    fn prefixes_do_not_determine_category() {
        assert_eq!(
            classify_category(&def("channel.feed.actual")).category,
            CatalogCategory::CncVariable
        );
        assert_eq!(
            classify_category(&def("channel.machine_axis_assignment")).category,
            CatalogCategory::MachineData
        );
        assert_eq!(
            classify_category(&def("channel.r_parameter")).category,
            CatalogCategory::SystemVariable
        );
    }
}
