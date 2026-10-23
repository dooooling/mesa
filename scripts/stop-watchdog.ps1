<#
.SYNOPSIS
  828D 一期首轮限定采样 —— 独立 watchdog（G6-ARMED 的执行体，也是 §5.1 实现要点的落地）。

.DESCRIPTION
  设计契约（先读，再改本文件）：
    docs/validation/828d-phase1-first-limited-sampling-application.md
      §4.6     G6-ARMED 七条判据（本脚本必须能逐条自证）
      §4.6.1   M0–M18 离线故障模拟（可重复实现见 tests/watchdog/）
      §5.1     实现要点 1–7（独立进程 / T0 自生成 / 独占 Start / 截止 Stop / 观测 / 双向解耦 / 退出码）
      §5.3     Stop 时间约束与"停干净"六项判据、Start/Stop 竞态处置

  ── 两种运行模式：默认 ARMED，永不自动进入 FIRED（P0-1 的落地）────────────────────
    arm（-Mode arm，**缺省值**）
        只做 ARMED 自检 + 报告，**零副作用**：不启动任何执行流、不写 t0.stamp、不发 Start，
        也不对端点做任何写操作（唯一写动作是显式 -SelfTest 时对**离线桩**做一次幂等
        POST …/stop，且要求端点本身已是 STOPPED 才做）。ARMED 之后脚本就结束了——
        旧版"等 1.5 s 就 FIRED 并发 Start"的行为已删除，等待再久也不会 Start。
    fire（-Mode fire）
        正式执行路径，必须**同时**满足（缺一项即 40，且不进入 FIRED）：
          ① -Authorized 开关 + -AuthorizedBy 授权人标识（显式授权、可追溯）；
          ② -AuthzTicket 授权凭证编号；
          ③ G6 现场签署：-G6Signoff + -G6SignedAt（时间必须可解析且不在未来）；
          ④ G7 部署身份：-G7DeployVersion 必须是 `sha256:<hex>` 或 `manifest:<path>`，
             **与部署二进制实算摘要比对**（git HEAD/describe 只作辅助信息，不再作为通过依据）；
             走 `manifest:` 形式时**还必须**给 -ExpectedManifestSha256（清单**自身**的 SHA-256，
             取自签署的 G6/G7 执行包），脚本在**解析清单内容之前**对清单**原始字节**算摘要比对（G1）；
          ⑤ 任务配置：-ExpectedTaskRevision + -ExpectedPointCount + -ApprovedTaskConfig
             + -ExpectedTaskFingerprint + **-ExpectedApprovedTaskConfigSha256**，
             脚本先在**解析获批配置文件之前**对文件**原始字节**算摘要、与签署值比对（G2），
             再**先做只读 GET**（diagnostics / tasks / 端点快照）与**实际完整配置比对**
             （连接 host/port/TSAP/PDU、逐点 Key 与参数、配置指纹），不一致或读不到即拒绝。

  ── 凭证完整性原则（G1/G2，本文件不可放宽）────────────────────────────────────────
    "被批准的对象本身必须与签署值比对"，否则校验可以被"把被批准对象一起换掉"绕过：
    · G1：只比"清单里的二进制摘要 ↔ 实际二进制"时，**清单与二进制一起被替换**仍然互相匹配；
      因此清单**自身**必须有一个来自**签署包**的独立摘要（-ExpectedManifestSha256）。
    · G2：任务配置指纹只覆盖**规范化 tasks 数组**，**不覆盖连接配置**；因此获批配置文件**自身**
      必须有一个来自签署包（-ExpectedApprovedTaskConfigSha256）。
    · 两者都在**解析内容之前**对**原始文件字节**算 SHA-256：一次 ReadAllBytes 的**同一份字节**
      既用于算摘要也用于后续解析，不存在"先解析再比对"或"比对与解析读到不同内容"（TOCTOU）的空间。
    · 缺失或不匹配 ⇒ 拒绝执行（exit 40，fail-closed），**不发 Start、不写 t0.stamp**。

  ── 授权边界（本轮冻结）───────────────────────────────────────────────────────────
    arm  只允许**回环地址且端口≠8132**（8132 是生产 mesad）：非回环或指向 8132 ⇒ 40，
         **无任何参数可绕过**。ARMED 永远不碰生产实例，自检只针对明确指定的离线模拟端点。
    fire 静态凭证通过后才放行任意 host（含 127.0.0.1:8132 生产 mesad、192.168.15.60 等
         现场地址）；任务配置核验的只读 GET 通过后，才允许进入 FIRED 并发 Start。
    **测试注入参数（B1）**：-TestHostProbeError / -TestAbortFlow 是离线测试专用，只允许配合
         明确的离线回环端点（loopback 且端口≠8132）；**非回环地址或生产端口 8132** 携带它们
         ⇒ 在**任何网络动作之前** exit 40（REFUSED_TEST_INJECTION_ONLINE，网络动作数 = 0）。
         原因：这两个参数会在 Start 之后**故意**中止工作流或伪造本机核对失败——把它们带进
         真实执行命令，等于我们自己制造异常，不能冒充"设备故障"或"无法确定"。
    40 = **授权边界拒绝，非设备故障**：它不是 Stop 失败，也不是采集异常，现场不要按设备
    故障处置（脚本会把理由与缺失/不匹配的凭证逐项打印并落盘）。

  ── 四条冻结行为（改动本文件前逐条自检）──────────────────────────────────────────
    F1 ARMED 零副作用：自检只针对明确指定的离线模拟端点；不自启动点、不改端点状态、不写 T0。
    F2 FIRED 必须显式触发：不得因"等待若干秒"启动端点；只有显式授权 + 四项凭证**校验通过**
       才进 FIRED。
    F3 退出码与试验结论分开：M1（观测阻塞但 Stop 准时）必须能同时表达"调度 PASS + 采集 INVALID"。
       退出码只表达"进程该怎么处置"，试验结论由 summary.json 的 verdict/axes 表达，二者不互相顶替。
    F4 停止时间不得夸大：T0+30 s 是 **Stop 请求的截止调度目标**，不是"驱动进程或设备通信已在
       第 30 秒物理终止"的保证；物理终止由"停干净"六项（含 HostChecks 三态）另行判定。
       因此 summary 里一律写作 deadline（调度目标），并另列 stops[].t_dispatch / convergence，
       禁止把 deadline 读成"第 30 秒已停稳"。

  本脚本的职责（fire 模式）：
    1) 由自己**独占**发出 Start，并在**同一时刻**用自身单调时钟生成 T0（写 t0.stamp）；
       **不接受**任何外部传入的 T0 或截止时间——没有这样的参数，也不读任何外部文件里的 T0。
    2) 在 T0 + 30 s **无条件**发出 Stop；30 s 是脚本内常量（$script:DEADLINE_OFFSET_MS），无参数入口。
    3) 每 200 ms 观测 GET /api/v1/endpoints/{id}，逐行追加 watchdog-states.jsonl。
    4) 以"**最终观测到** STOPPED + desired_running=false + 本机核对收敛"（而不是"Stop 已发出"）
       给出退出码，并把"调度是否准时"与"采集是否有效"分开判定。

  ── 计时/Stop 与观测解耦（G6-ARMED-3，本文件的核心）────────────────────────────────
  五个**独立执行流**（同一进程内的独立 runspace，各自独立线程，互不等待、互不 join）：

      OBS   观测流：GET 端点、写 jsonl、判定 S1/S2/S3 触发条件。可以被 HTTP 阻塞任意久。
      TIMER 计时流：**只读单调时钟**，从不读端点状态、从不发 HTTP；到 T0+30 s 置信号。
      STOP  停止流：等信号后**发出** Stop 请求；Stop 自身可能挂 75–80 s，不阻塞任何人。
      START 启动流：等放行信号后发 Start；Start 可能 10 s 超时，不阻塞主流程。
      MON   监护流（P0-2 / M10）：**独立于 OBS 与 TIMER**，20 ms 粒度做两件事——
              a) 观测心跳：距最近一次**成功**样本超过 -ObserveStallSec（默认 3 s）即判观测通道
                 失效 → 入队 OBS_STALL **提前 Stop**，不等被阻塞的 GET 返回；
              b) 工作流存活：任一工作流异常退出（TIMER/OBS/STOP/START）→ 记 WORKER_ABORT、
                 立即 Stop；TIMER 死亡时**接管截止调度**（DEADLINE_FAILSAFE），
                 保证"工作流挂了也不会静默继续"。

  主流程（协调者）只做"读共享样本 → 判定触发 → 入队"，**自己从不发 HTTP**。
  因此：任一条 HTTP 请求（Start / Stop / GET）挂起，都不会推迟 T0+30 s 的 Stop 发出；
  反过来 Stop 挂住时观测也照常进行（双向解耦，§5.1-6）。
  关键点：**TIMER 流不读端点状态**，所以"何时 Stop"不依赖被测流程的循环或状态。

  ── HostChecks 三态（P0-3）────────────────────────────────────────────────────────
  fire 模式**默认强制**本机核对（驱动进程 / 到设备的 TCP 连接），不再默认关闭：
    · 只认目标驱动：进程按 driver_id 匹配（mesa-driver-<driver_id> 或 --driver-id <id>），
      其它 mesa-driver-* 进程单列为 excluded，**不**计入"本次驱动"；
    · 只认目标端口：只统计到 device host **且** device port 的 ESTABLISHED；现场的 3389/RDP
      连接单独统计并显式标注"已排除"，绝不与采集连接混算；
    · 判定三态：converged（已收敛）/ not_converged（仍未收敛）/ indeterminate（无法确定）。
      **查询失败、权限不足、工具缺失、结果不确定一律判 indeterminate**，不得判"停净"。
  -SkipHostChecks 允许显式跳过，但该项在 verdict/退出码上体现为 not_verified（退出码 7），
  永远不可能被判成"停干净"。

.PARAMETER Mode
  arm（缺省）| fire。见上面"两种运行模式"。**没有任何参数能表达"arm 之后自动 fire"**。

.PARAMETER Authorized
  fire 模式的显式授权开关（必须显式给出）。

.PARAMETER AuthorizedBy
  授权人标识（fire 必填）：授权必须可追溯到人，脚本只校验非空并记录。

.PARAMETER AuthzTicket
  授权凭证编号（工单/G6 放行单号），fire 必填；只校验非空并记录，不校验真伪。

.PARAMETER G6Signoff
  G6 现场签署标识（签署人/签署编号），fire 必填。

.PARAMETER G6SignedAt
  G6 签署时间（ISO 8601，如 2026-10-24T09:00:00+08:00），fire 必填；
  脚本校验**可解析且不在未来**。

.PARAMETER G7DeployVersion
  G7 部署身份确认（fire 必填），**只接受**两种可独立证明"实际部署二进制身份"的形式：
    · `sha256:<64 hex>`  —— 与 -G7BinaryPath（缺省探测 target/{release,debug}/mesad[.exe]）**实算**摘要比对；
    · `manifest:<path>`  —— 从受信构建清单读期望摘要，再与部署二进制实算摘要比对。
  **git commit SHA / git describe 不再作为通过依据**（只能证明"输入值与仓库版本匹配"，不能证明
  部署物就是该版本构建的）；它们只作为辅助信息记录在 summary/t0.stamp 里。
  两种形式都不可用（未给摘要/清单、清单不可读、定位不到二进制）⇒ `unverified` ⇒ 拒绝进入 FIRED
  （fail-closed，**不得**降级为"用 git HEAD 代替"）。

.PARAMETER G7BinaryPath
  部署二进制路径（可选）。给出时**只**比对这个文件；给了却不存在 ⇒ 直接 unverified
  （不回退到 target/ 下的其它二进制，避免"凑出通过"）。缺省时按
  `target/release/mesad.exe` → `target/release/mesad` → `target/debug/mesad.exe` → `target/debug/mesad`
  顺序探测，并如实记录最终比对的是哪一个文件。

.PARAMETER ExpectedManifestSha256
  受信构建清单**自身**的 SHA-256（`sha256:<64 hex>`，也接受裸 64 hex）。**走 `manifest:` 形式时必填**
  （G1：清单完整性）。取值**只能来自已签署的 G6/G7 执行包**，禁止在现场用 `Get-FileHash` 现算——
  自己现算等于自己给自己授权，与本参数的目的（防"清单+二进制一起被替换"）正好相反。
  校验时机与方式：
    · 在**解析清单内容之前**对清单**原始文件字节**算 SHA-256（一次 ReadAllBytes，摘要与解析共用
      同一份字节，避免"解析后再比对"或 TOCTOU 绕过）；
    · 缺失 ⇒ `REFUSED_MANIFEST_INTEGRITY`（exit 40）；不匹配 ⇒ 同上（exit 40）；格式非法 ⇒ 同上；
    · 清单读不到 / 不是合法 JSON ⇒ `unverified` ⇒ 同样拒绝（fail-closed）。
  给了本参数但 `-G7DeployVersion` 不是 `manifest:<path>` ⇒ **拒绝**（不静默忽略一个已批准的摘要）。

.PARAMETER ExpectedTaskRevision
  期望的任务配置 revision（fire 必填）。脚本先做只读任务快照
  （`GET /api/v1/tasks?endpoint={EndpointId}`，404/405 回退 `GET /api/v1/tasks/{EndpointId}`），
  与返回的 `revision` 比对，不一致即拒绝。

.PARAMETER ExpectedPointCount
  **获批任务配置的点数**（fire 必填，0 表示"获批配置应为 0 个点"）。取值来自签署包，不是现场现算。
  核验方式（BLOCKED-01 修订）：脚本只读 `GET /api/v1/tasks?endpoint={id}`（404/405 回退
  `GET /api/v1/tasks/{id}`）取**实时任务快照**，按 `tasks[].binding.config.points[]` 求和得到
  **当前任务点数**，与本参数比对；不一致 ⇒ `REFUSED_TASKCONFIG_MISMATCH`（exit 40）。
  **不再**用 `diagnostics.point_count` 作判据：该值来自持久化 `point_registry` 行数
  （`core-api:1499` → `config-store:2459-2473`），唯一写点是 `assign_point_ids`
  （`config-store:2344-2456`，由 `run_config_flow:943-945` 在 ConfigureTasks 之后写入），
  因此它反映的是"**上次走完 ConfigureTasks 的那次 Start**"的残留行数，与当前任务配置无关，
  用它判"配置正确性"等于拿历史残留冒充当前配置。
  该值仍会被读取并写进证据（`task_config.actual.diagnostics_point_count` 与
  `task_config_observation.diagnostics_point_count_observed`），**只作运行状态观测**。

.PARAMETER ApprovedTaskConfig
  获批任务配置 JSON 路径（fire 必填）。文件里给出连接配置（host / port / local_tsap / remote_tsap /
  pdu_length）与**完整点位清单**（tasks[].binding.config.points[]：Key 与全部参数）。
  脚本把实际只读 GET 到的值与之**逐项、逐点**比对（B3：只比 revision/点数不足以证明与获批配置一致）。

.PARAMETER ExpectedTaskFingerprint
  批准版本的任务配置指纹（fire 必填，`sha256:<64 hex>`）。指纹 = SHA-256(规范化 tasks 数组)，
  规范化 = 键递归排序 + 按 task.id 排序 + 紧凑 JSON。脚本会用同一算法核对：
  ① 获批配置文件本身的指纹必须等于该值（防止"拿错/被改过的批准文件"）；
  ② 实际只读 GET 到的 tasks 指纹必须等于该值。
  **注意指纹不覆盖连接配置**：connection（host/port/TSAP/PDU）不在规范化 tasks 数组内，
  所以"指纹不变"**不能**证明文件未被改动 —— 那必须靠 -ExpectedApprovedTaskConfigSha256（G2）。

.PARAMETER ExpectedApprovedTaskConfigSha256
  获批任务配置**文件自身**的 SHA-256（`sha256:<64 hex>`，也接受裸 64 hex）。fire 必填
  （G2：被批准对象自身完整性）。取值**只能来自已签署的 G6/G7 执行包**，禁止在现场现算。
  校验时机与方式：
    · 在**解析文件之前**对文件**原始字节**算 SHA-256（一次 ReadAllBytes，摘要与解析共用同一份字节）；
    · 该检查发生在**任何网络动作之前**（静态凭证阶段），因此拒绝时 network_actions = 0；
    · 缺失 / 不匹配 / 格式非法 ⇒ `REFUSED_TASKCONFIG_INTEGRITY`（exit 40）；文件读不到 ⇒ `unverified` ⇒ 拒绝。
  为什么必须独立于 -ExpectedTaskFingerprint：指纹只覆盖规范化 tasks 数组，**connection 被改动时指纹不变**，
  只有文件摘要能挡住这种篡改。

.PARAMETER TaskConfigRef
  可选自由文本备注（任务配置核验依据的说明/工单链接），只记录不校验；
  真正的核验由 -ExpectedTaskRevision + -ExpectedPointCount + -ApprovedTaskConfig
  + -ExpectedTaskFingerprint 共同完成。

.PARAMETER BaseUrl
  Core 的 REST 根地址。arm：**只允许回环且端口≠8132**；fire：凭证校验通过后允许任意 host。
  非回环/生产端口在 arm 下一律 40 拒绝，不建立任何连接。

.PARAMETER ObserveTimeoutSec
  观测 GET 的**客户端**超时，生产默认 5 s。
  M1（同步阻塞）必须放宽到大于阻塞时长（如 45），否则只是"超时兜底"、不构成解耦证明（§4.6.1）。

.PARAMETER ObserveMaxSec
  观测总上限，默认 120 s（§5.1-5）。到点即停观测，不延长。
  **该上限由主流程独立计时**（20 ms 粒度），不依赖任何 HTTP 请求返回。

.PARAMETER ObserveStallSec
  观测心跳阈值（秒），默认 3，0 = 关闭。
  >0 时由 MON 流独立监测"距最近一次**成功**样本的时长"；超过阈值即判观测通道失效，
  **提前发 Stop**（原因 OBS_STALL），不等被阻塞的 GET 返回，并把采集判为 INVALID。
  关闭它只用于"纯调度解耦"模拟（M1）：那种场景的采样证据必然无效，判据仍会把它判为 INVALID。

.PARAMETER OutDir
  证据目录。正式执行必须显式给 target\validation\828d-r1\round1-<UTC 时间戳>（§6.1）。

.PARAMETER SelfTest
  G6-ARMED-4 通路自检：对**当前已 STOPPED** 的端点调用一次 POST …/stop，要求 200 且
  was_running=false，写 stop-selftest.json。只在 arm 模式可用（fire 之前由人工单独跑一次）。
  自检不生成 t0.stamp、不发 Start。

.PARAMETER HostChecks
  兼容旧命令行：fire 模式默认已强制 HostChecks，本开关无副作用（保留仅为不改旧调用方式）。

.PARAMETER SkipHostChecks
  显式跳过本机核对（④驱动进程 / ⑤到设备 TCP）。跳过后这两项记为 not_verified，
  **不得**判"停净"：verdict = HOSTCHECKS_UNVERIFIED，退出码 7。

.PARAMETER ArmHoldSec
  arm 模式在报告前保持的秒数（默认 2 s）。用于演示"即使等待超过旧版的 1.5 s 也不会 FIRED"（M7）。

.EXAMPLE
  # ① ARMED 自检（零副作用；结束时打印"是否可以进入 FIRED"与每项凭证的校验结果）
  pwsh -NoProfile -File scripts/stop-watchdog.ps1 -Mode arm `
       -BaseUrl http://127.0.0.1:18080 -OutDir target\validation\stop-watchdog\M7 `
       -G6Signoff "张工/QC-828D-01" -G6SignedAt "2026-10-24T09:00:00+08:00" `
       -G7DeployVersion "sha256:<部署 mesad 的 sha256>" -G7BinaryPath "C:\mesa\bin\mesad.exe" `
       -ExpectedTaskRevision rev13 -ExpectedPointCount 12 `
       -ApprovedTaskConfig "target\validation\approved-task-config.json" `
       -ExpectedApprovedTaskConfigSha256 "sha256:<获批配置文件的 sha256>" `
       -ExpectedTaskFingerprint "sha256:<规范化 tasks 数组的 sha256>"

.EXAMPLE
  # ② 正式执行（fire）：显式授权 + 授权人 + 四项凭证（全部**校验**通过才放行）
  #    注意两个"被批准对象自身"的摘要都**取自签署包**，不得在现场现算：
  #      -ExpectedManifestSha256（清单自身，G1）与 -ExpectedApprovedTaskConfigSha256（获批配置自身，G2）
  pwsh -NoProfile -File scripts/stop-watchdog.ps1 -Mode fire -Authorized -AuthorizedBy "李工" `
       -AuthzTicket G6-828D-R1 -G6Signoff "张工/QC-828D-01" -G6SignedAt "2026-10-24T09:00:00+08:00" `
       -G7DeployVersion "manifest:target\validation\mesad-build-manifest.json" `
       -G7BinaryPath "C:\mesa\bin\mesad.exe" `
       -ExpectedManifestSha256 "sha256:<受信构建清单自身的 sha256（来自签署包）>" `
       -ExpectedTaskRevision rev13 -ExpectedPointCount 12 `
       -ApprovedTaskConfig "target\validation\approved-task-config.json" `
       -ExpectedApprovedTaskConfigSha256 "sha256:<获批配置文件的 sha256（来自签署包）>" `
       -ExpectedTaskFingerprint "sha256:<规范化 tasks 数组的 sha256>" `
       -BaseUrl http://127.0.0.1:8132 -OutDir target\validation\828d-r1\round1-20261024T010203Z

.EXAMPLE
  # ③ 离线 M1：假端点 GET 挂起 36 s，观测超时放宽到 45 s（心跳关闭，只测调度解耦）
  pwsh -NoProfile -File scripts/stop-watchdog.ps1 -Mode fire -Authorized -AuthorizedBy SIM `
       -AuthzTicket SIM-M1 -G6Signoff SIM-G6 -G6SignedAt "2026-10-24T09:00:00+08:00" `
       -G7DeployVersion "sha256:<离线测试二进制摘要>" -G7BinaryPath "target\validation\fixtures\mesad-fixture.bin" `
       -ExpectedTaskRevision rev13 -ExpectedPointCount 12 `
       -ApprovedTaskConfig "target\validation\approved-task-config.json" `
       -ExpectedApprovedTaskConfigSha256 "sha256:<获批配置文件的 sha256>" `
       -ExpectedTaskFingerprint "sha256:<规范化 tasks 数组的 sha256>" `
       -BaseUrl http://127.0.0.1:18081 -ObserveTimeoutSec 45 -ObserveStallSec 0 `
       -OutDir target\validation\stop-watchdog\M1

.NOTES
  严禁指向真实机床：离线模拟只用 127.0.0.1 的假端点或不可达地址（脚本本身也会拒绝非回环地址）。
  本脚本**不 import** 任何驱动/Core 代码，只经 REST 交互（§5.1-1）。
  可重复的离线测试框架在 tests/watchdog/（一条命令跑完 M0–M18）。

  ── 退出码（进程怎么处置）与试验结论（summary.json 的 verdict / axes）分开 ──────────
  arm 模式：
    0  ARMED_READY                 自检通过 + 四项凭证全部校验通过（= 具备进入 FIRED 的条件，但未执行）
    1  ARMED_INPUTS_MISSING        自检通过但凭证缺失/校验未通过 → 不得进入 FIRED
    2  ARMED_SELFTEST_FAIL         通路自检失败（-SelfTest 未拿到 200 + was_running=false）
    3  ARMED_ENDPOINT_UNREACHABLE  端点不可达（离线桩未起 / 地址错）
  拒绝执行（授权边界，**先于一切写动作**，也先于 FIRED）：
    40 REFUSED_NON_OFFLINE         arm 指向非回环地址或生产 mesad 端口 8132
    40 REFUSED_TEST_INJECTION_ONLINE 携带 -TestHostProbeError/-TestAbortFlow 却指向非回环地址或端口 8132
                                    （B1：在任何网络动作之前拒绝，网络动作数 = 0）
    40 REFUSED_NOT_AUTHORIZED      fire 缺 -Authorized / -AuthorizedBy / -AuthzTicket
    40 REFUSED_CREDENTIAL_INVALID  G6 签署时间不可解析；G7 部署二进制摘要与期望不一致
    40 REFUSED_MANIFEST_INTEGRITY  受信清单**自身**未被独立验证（G1）：缺 -ExpectedManifestSha256、
                                    清单原始字节摘要与签署值不一致、格式非法、或给了该参数却不走 manifest 形式
                                    （本地检查，**网络动作数 = 0**）
    40 REFUSED_TASKCONFIG_INTEGRITY 获批任务配置**文件自身**未被独立验证（G2）：缺
                                    -ExpectedApprovedTaskConfigSha256、文件原始字节摘要与签署值不一致、格式非法
                                    （本地检查，**网络动作数 = 0**；指纹相同也不放行）
    40 REFUSED_TASKCONFIG_MISMATCH 完整任务配置与获批配置不一致（连接配置 / 点位 Key 与参数 / 指纹 / revision / 当前任务点数）
    40 REFUSED_CREDENTIAL_UNVERIFIABLE 凭证读不到或无法独立验证（G7 无摘要/清单、部署二进制定位不到、
                                    端点读不到、获批配置不可用）→ 按未通过处理（fail-closed）
  **40 一律表示"授权边界拒绝，非设备故障"**：未发出 Start、未进入 FIRED。
  fire 模式：
    0  STOPPED_CLEAN               收敛 CONVERGED + 采集 VALID + Start 确认 + 无竞态 + 无提前触发
    1  NOT_STOPPED                 收敛 NOT_CONVERGED（最高严重级，已打印 §5.3 隔离处置）
    2  STOPPED_OBS_INVALID         停住了，但采集证据无效（观测断流/心跳失效/超限/无样本）
    3  STOPPED_START_UNCERTAIN     停住了，但 Start 结果不确定（在途/超时），生命周期未确认
    4  STOPPED_WITH_RACE           停住了，但记录到 RACE_START_AFTER_STOP
    5  STOPPED_BY_EARLY_TRIGGER    停住了，但 Stop 是被 S1/S2/S3/S6 提前触发的
    6  STOPPED_WITH_ANOMALIES      停住了，但上述 2–5 至少命中两项
    7  HOSTCHECKS_UNVERIFIED       HostChecks 被显式跳过 → "停干净"未验证（不是"已停净"）
    8  WORKER_ABORTED              监护流发现工作流异常退出 → 本轮作废，须人工处置
    9  CONVERGENCE_INDETERMINATE   本机核对无法确定（查询失败/权限不足），不得判停净
  **只有 0 可被当作"本轮可判 PASS"**；2–9 一律"停住了/不确定，本轮不计 PASS"。
  优先级：40（授权边界拒绝）> 8（工作流异常）> 1（已观测到仍未收敛）> 2（观测中断→采集无效）
          > 9（收敛无法确定）> 7（HostChecks 未验证）> 6（多异常）> 3–5。
  （"观测中断"排在"收敛无法确定"之前：前者是根因，也是 M8 需要能被单独看见的那一档。）
  另见 summary.json 的 `trial`（试验结论：PASS / NO_GO / INVALID_SAMPLING / UNVERIFIED /
  ABORTED / NO_GO_ANOMALY）与 `axes`（schedule / sampling / convergence 三条独立判定轴）。
#>
[CmdletBinding()]
param(
    [ValidateSet('arm', 'fire')][string]$Mode = 'arm',
    [switch]$Authorized,
    [string]$AuthzTicket = '',
    [string]$AuthorizedBy = '',
    [string]$G6Signoff = '',
    [string]$G6SignedAt = '',
    [string]$G7DeployVersion = '',
    [string]$G7BinaryPath = '',
    # G1：受信构建清单**自身**的 SHA-256（必须来自签署的 G6/G7 执行包；manifest 形式时必填）
    [string]$ExpectedManifestSha256 = '',
    [string]$ExpectedTaskRevision = '',
    [ValidateRange(-1, 10000000)][int]$ExpectedPointCount = -1,
    [string]$ApprovedTaskConfig = '',
    [string]$ExpectedTaskFingerprint = '',
    # G2：获批任务配置**文件自身**的 SHA-256（必须来自签署的 G6/G7 执行包；fire 必填）
    [string]$ExpectedApprovedTaskConfigSha256 = '',
    [string]$TaskConfigRef = '',
    [string]$BaseUrl = 'http://127.0.0.1:8132',
    [string]$EndpointId = 'lx-nck-tags-190-connection',
    [string]$OutDir,
    [ValidateRange(1, 600)][int]$ObserveTimeoutSec = 5,
    [ValidateRange(10, 3600)][int]$ObserveMaxSec = 120,
    [ValidateRange(1, 600)][int]$StopTimeoutSec = 120,
    [ValidateRange(1, 120)][int]$StartTimeoutSec = 10,
    [ValidateRange(0, 300)][int]$ObserveStallSec = 3,
    [ValidateRange(1, 120)][int]$ArmHoldSec = 2,
    [switch]$SelfTest,
    [switch]$HostChecks,
    [switch]$SkipHostChecks,
    [string]$Tag = 'run',
    # ── 以下是**离线测试专用**注入开关（tests/watchdog/ 使用）──────────────────────────
    # 只用于复现"查询失败 / 工作流异常退出"两类故障；正常执行时全部为缺省值。
    # B1 冻结：这两个参数**只允许**配合明确的离线回环端点（loopback 且端口≠8132）；
    # 非回环地址或生产端口 8132 携带它们 ⇒ 在**任何网络动作之前** exit 40
    # （REFUSED_TEST_INJECTION_ONLINE，见下面的 Test-WdInjectionGuard）。
    [ValidateSet('', 'process', 'tcp', 'both')][string]$TestHostProbeError = '',
    [ValidateSet('', 'obs', 'timer', 'stop', 'start')][string]$TestAbortFlow = '',
    [ValidateRange(1, 120)][int]$TestAbortAfterSec = 3
)

$ErrorActionPreference = 'Stop'

#region ── 常量（硬性：30 s 截止为脚本内常量，不提供参数入口）─────────────────────
# §4.6-G6-ARMED-2：截止偏移 30 s 以常量固化在脚本内；§4.6："截止时刻 = watchdog 自己记录的
# T0 + 30 s，不接受外部（人工、主流程、抓证脚本）指定或修正"。故此处**故意**不是参数。
$script:DEADLINE_OFFSET_MS = 30000
# §5.1-5：每 200 ms 观测一次。
$script:OBSERVE_INTERVAL_MS = 200
# §5.3-⑥：自首次观测到 STOPPED 起连续 10 s 无新 epoch、无 RECONNECTING。
$script:SETTLE_MS = 10000
# §5.3：Stop 发出后若见 RUNNING / epoch 变化，再 Stop，最多 3 次。
$script:MAX_STOPS = 3
# §7-S1/S2：端点 detail 里的通信失败特征（首次确认即停，不设"连续 N 次"宽限）。
$script:COMM_FAIL_PATTERN = '(?i)(S7_JOB_0x8201|S7_JOB_[0-9A-Fa-f]{4}|COTP_RECV_FAIL|COTP_SEND_FAIL|early eof|CONNECT_TIMEOUT|POLL_FAIL|RECV_FAIL|SEND_FAIL|CONNECT_FAIL)'
# "采样有效"判据：30 s 窗口里若出现超过该值的观测盲区，S1/S2/S3 可能被整段漏掉，
# 该轮采样证据一律判 INVALID（与"停住了"分开判定，见文件头退出码语义）。
# 取 2 s：正常 200 ms 轮询的最大间隔实测 <100 ms，2 s 已是 20 倍余量，不会误伤正常抖动。
$script:SAMPLING_MAX_GAP_MS = 2000
# MON 监护流的检查粒度（P0-2：心跳阈值默认 3 s，20 ms 粒度把"发现失效"压到阈值 +1 个 tick）。
$script:MONITOR_INTERVAL_MS = 20
# 本轮修订的授权边界：只允许回环地址（见文件头"离线授权边界"）。**不是**可被参数绕过的开关。
$script:OFFLINE_ONLY = $true
# 任务配置核验期间发出的只读 GET 计数（用于拒绝时如实报告"网络动作次数"）。
$script:credNetworkActions = 0
#endregion

#region ── 跨执行流共享的辅助函数 ─────────────────────────────────────────────────
# 这些函数在"主流程"和"子 runspace"里都要用。子 runspace 有独立的函数表（不能直接调父作用域的
# 函数），因此把函数体放在一个字符串里：主流程 Invoke-Expression 一次，子流把同一段文本拼在
# 自己脚本前面编译。这样只有一份实现，不会出现"日志格式两处不一致"。
$script:CommonPrelude = @'
function Add-WdLog {
    param($Ctx, [string]$Level, [string]$Message)
    $ts = (Get-Date).ToString('o')
    $line = "- ``{0}`` [{1}] {2}" -f $ts, $Level, $Message
    [System.Threading.Monitor]::Enter($Ctx.logLock)
    try {
        [System.IO.File]::AppendAllText($Ctx.logPath, $line + "`n", [System.Text.UTF8Encoding]::new($false))
    }
    finally { [System.Threading.Monitor]::Exit($Ctx.logLock) }
}

# 追加一行 JSON（jsonl）。多线程写同一文件，用 logLock 串行化，避免行被截断。
function Write-WdJsonl {
    param($Ctx, [string]$Path, $Record)
    $line = ($Record | ConvertTo-Json -Compress -Depth 8)
    [System.Threading.Monitor]::Enter($Ctx.logLock)
    try {
        [System.IO.File]::AppendAllText($Path, $line + "`n", [System.Text.UTF8Encoding]::new($false))
    }
    finally { [System.Threading.Monitor]::Exit($Ctx.logLock) }
}

# 单调时钟（毫秒）。整个脚本只有一个 Stopwatch 实例，所有执行流读同一个时间基准。
function Get-WdMono {
    param($Ctx)
    return [double]$Ctx.sw.Elapsed.TotalMilliseconds
}
'@
Invoke-Expression $script:CommonPrelude
#endregion

#region ── 运行目录与日志骨架 ─────────────────────────────────────────────────────
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path 'target\validation' ('watchdog-' + (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ'))
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path

$runLogPath = Join-Path $OutDir 'run-log.md'
$t0Path = Join-Path $OutDir 't0.stamp'
$statesPath = Join-Path $OutDir 'watchdog-states.jsonl'
$stopsPath = Join-Path $OutDir 'watchdog-stops.jsonl'
$summaryPath = Join-Path $OutDir 'summary.json'
$selfTestPath = Join-Path $OutDir 'stop-selftest.json'
$armReportPath = Join-Path $OutDir 'arm-report.json'

$procStart = Get-Date
$hostName = [System.Net.Dns]::GetHostName()

$header = @(
    '# stop-watchdog 运行日志'
    ''
    "- 脚本：``scripts/stop-watchdog.ps1``"
    "- PID：$PID（独立进程；本行即 G6-ARMED-1 的 PID 证据）"
    "- 进程启动时刻（本地）：$($procStart.ToString('o'))"
    "- PowerShell：$($PSVersionTable.PSVersion)"
    "- 主机：$hostName"
    "- 模式：Mode=$Mode（arm = 只自检、零副作用；fire = 显式授权后的正式执行）"
    "- 参数：BaseUrl=$BaseUrl EndpointId=$EndpointId ObserveTimeoutSec=$ObserveTimeoutSec ObserveMaxSec=$ObserveMaxSec StopTimeoutSec=$StopTimeoutSec StartTimeoutSec=$StartTimeoutSec ObserveStallSec=$ObserveStallSec ArmHoldSec=$ArmHoldSec SelfTest=$SelfTest HostChecks=$HostChecks SkipHostChecks=$SkipHostChecks Tag=$Tag"
    "- 授权：Authorized=$Authorized AuthorizedBy=$(if ($AuthorizedBy) { $AuthorizedBy } else { '（未提供）' }) AuthzTicket=$(if ($AuthzTicket) { $AuthzTicket } else { '（未提供）' })"
    "- 现场输入：G6Signoff=$(if ($G6Signoff) { $G6Signoff } else { '（未提供）' })；G6SignedAt=$(if ($G6SignedAt) { $G6SignedAt } else { '（未提供）' })；G7DeployVersion=$(if ($G7DeployVersion) { $G7DeployVersion } else { '（未提供）' })；ExpectedManifestSha256=$(if ($ExpectedManifestSha256) { $ExpectedManifestSha256 } else { '（未提供）' })；ExpectedTaskRevision=$(if ($ExpectedTaskRevision) { $ExpectedTaskRevision } else { '（未提供）' })；ExpectedPointCount=$ExpectedPointCount；ExpectedApprovedTaskConfigSha256=$(if ($ExpectedApprovedTaskConfigSha256) { $ExpectedApprovedTaskConfigSha256 } else { '（未提供）' })；TaskConfigRef=$(if ($TaskConfigRef) { $TaskConfigRef } else { '（未提供）' })"
    "- 截止偏移：$($script:DEADLINE_OFFSET_MS) ms（脚本内常量，不接受外部指定）"
    '- 说明：T0 与截止时刻**只**由本进程在 FIRED 时刻生成；人工/主流程/抓证脚本不得预先填写或修正。'
    '- 说明：T0+30 s 是 **Stop 请求的截止调度目标**，不代表驱动进程/设备通信已在第 30 s 物理终止。'
    ''
    '## 事件'
    ''
) -join "`n"
[System.IO.File]::WriteAllText($runLogPath, $header + "`n", [System.Text.UTF8Encoding]::new($false))

# 共享上下文：所有字段在子流启动前一次性建好（子流只读键名、不新增键）。
$ctx = @{
    baseUrl            = $BaseUrl.TrimEnd('/')
    endpointId         = $EndpointId
    observeTimeoutSec  = $ObserveTimeoutSec
    observeMaxSec      = $ObserveMaxSec
    observeStallSec    = $ObserveStallSec
    samplingMaxGapMs   = $script:SAMPLING_MAX_GAP_MS
    stopTimeoutSec     = $StopTimeoutSec
    startTimeoutSec    = $StartTimeoutSec
    observeIntervalMs  = $script:OBSERVE_INTERVAL_MS
    settleMs           = $script:SETTLE_MS
    maxStops           = $script:MAX_STOPS
    monitorIntervalMs  = $script:MONITOR_INTERVAL_MS
    sw                 = [System.Diagnostics.Stopwatch]::StartNew()
    logLock            = [object]::new()
    logPath            = $runLogPath
    statesPath         = $statesPath
    stopsPath          = $stopsPath
    armed              = [System.Threading.ManualResetEventSlim]::new($false)
    startGo            = [System.Threading.ManualResetEventSlim]::new($false)
    obsDone            = [System.Threading.ManualResetEventSlim]::new($false)
    shutdown           = [System.Threading.ManualResetEventSlim]::new($false)
    wake               = [System.Threading.ManualResetEventSlim]::new($false)
    stopSignal         = [System.Threading.SemaphoreSlim]::new(0)
    stopQueue          = [System.Collections.Concurrent.ConcurrentQueue[string]]::new()
    history            = [System.Collections.Concurrent.ConcurrentQueue[object]]::new()
    stopRecords        = [System.Collections.Concurrent.ConcurrentQueue[object]]::new()
    monEvents          = [System.Collections.Concurrent.ConcurrentQueue[object]]::new()
    d                  = [System.Collections.Concurrent.ConcurrentDictionary[string, object]]::new()
    flowHandles        = @()
}

# 晚发布的共享标量一律放 $ctx.d（并发字典），避免多线程写普通哈希表。
foreach ($k in @('t0Mono', 'deadlineMono', 't0UnixNs', 'deadlineFireMono', 'stopFirstMono',
        'stopCount', 'stopInFlight', 'startDispatchMono', 'startStatus',
        'startElapsedMs', 'settleOk', 'settleSamples', 'settleResets', 'obsSamples',
        'obsFailures', 'obsMaxReqMs', 'obsFirstStoppedMono', 'devicePort', 'raceCount',
        'postStopEpoch', 'stopLastMono', 'lastSuccessMono', 'maxSuccessGapMs',
        'obsStallFired', 'obsTruncated', 'earlyTriggered', 'obsAborted', 'timerDead',
        'deadlineFailsafeFired', 'workerAbortCount', 'monTicks')) {
    $ctx.d[$k] = 0
}
$ctx.d['startOutcome'] = 'pending'
$ctx.d['startErr'] = ''
$ctx.d['startBody'] = ''
$ctx.d['startDispatchLocal'] = ''
$ctx.d['deviceHost'] = ''
$ctx.d['driverId'] = ''
$ctx.d['postStopEpoch'] = ''
$ctx.d['settleOk'] = $false
$ctx.d['obsStallFired'] = $false
$ctx.d['obsTruncated'] = $false
$ctx.d['earlyTriggered'] = $false
$ctx.d['obsAborted'] = $false
$ctx.d['timerDead'] = $false
$ctx.d['deadlineFailsafeFired'] = $false
$ctx.d['obsExited'] = $false
$ctx.d['workersAborted'] = @()

Add-WdLog $ctx 'INFO' "启动：PID=$PID Mode=$Mode BaseUrl=$($ctx.baseUrl) EndpointId=$EndpointId OutDir=$OutDir"
Add-WdLog $ctx 'INFO' "常量：截止偏移=$($script:DEADLINE_OFFSET_MS) ms 观测间隔=$($script:OBSERVE_INTERVAL_MS) ms 心跳阈值=$ObserveStallSec s 稳定窗口=$($script:SETTLE_MS) ms Stop 上限=$($script:MAX_STOPS) 次"
Write-Host ("[start] PID={0}  Mode={1}  BaseUrl={2}  Endpoint={3}  OutDir={4}" -f $PID, $Mode, $ctx.baseUrl, $EndpointId, $OutDir)

#region ── 授权边界（P0-1）：arm 只允许离线回环；fire 必须四项凭证校验通过才放行 ──────
# 冻结规则：
#   arm（缺省） 允许范围 = 回环地址（127.0.0.0/8 / ::1 / localhost）**且端口不是 8132**。
#               非回环 ⇒ 40；127.0.0.1:8132（生产 mesad）⇒ 40。**没有任何参数可绕过**：
#               ARMED 永远不碰生产实例，只针对明确指定的离线模拟端点。
#   fire        静态凭证（-Authorized/-AuthorizedBy/-AuthzTicket + G6 + G7）校验通过后，
#               才允许任意 host（含生产 mesad 8132 与 192.168.15.60 等现场地址）；
#               任务配置还要**先做只读 GET**（任务快照 + 端点连接配置）与获批配置比对，
#               不一致或读不到 ⇒ 40，且**绝不进入 FIRED**（不发 Start）。
# 40 = "授权边界拒绝，**非设备故障**"：它不是 Stop 失败、也不是采集异常，现场不要按设备故障处置。
$script:PRODUCTION_CORE_PORT = 8132

function Get-WdUrlInfo {
    # 解析 BaseUrl，供边界判断使用（host / port / 是否回环）。
    param([string]$Url)
    $u = $null
    try { $u = [System.Uri]::new($Url) } catch { }
    if (-not $u) {
        return [ordered]@{ ok = $false; scheme = ''; host = $Url; port = 0; loopback = $false; reason = "BaseUrl 不是合法 URI：$Url" }
    }
    if ($u.Scheme -ne 'http' -and $u.Scheme -ne 'https') {
        return [ordered]@{ ok = $false; scheme = $u.Scheme; host = $u.Host; port = $u.Port; loopback = $false; reason = "只接受 http/https：$($u.Scheme)" }
    }
    $h = $u.Host.Trim('[', ']')
    $port = if ($u.Port -gt 0) { $u.Port } elseif ($u.Scheme -eq 'https') { 443 } else { 80 }
    $loopback = $false
    if ($h -eq 'localhost' -or $h -eq '::1' -or $h -eq '0.0.0.0') { $loopback = $true }
    elseif ($h -match '^127\.') { $loopback = $true }
    return [ordered]@{
        ok       = $true
        scheme   = $u.Scheme
        host     = $h
        port     = $port
        loopback = $loopback
        reason   = $(if ($loopback) { '回环/本机地址' } else { "非回环地址 $h" })
    }
}

function Get-WdProp {
    # 空安全取属性：$null 对象 / 缺失键一律返回 $null（不抛错）。
    # 为什么需要：证据里 `items.task_config.actual` 在"静态阶段未做网络核验"时就是 $null，
    # 而拒绝记录与汇总代码要无条件读取其中的观测字段 —— 用本函数避免"某个阶段少一个字段就整轮崩"。
    # 位置说明：必须定义在**第一次调用点之前**（Write-WdRefusal 在授权边界拒绝时就会被调用）。
    param($Obj, [string]$Name)
    if ($null -eq $Obj) { return $null }
    if ($Obj -is [System.Collections.IDictionary]) {
        if ($Obj.Contains($Name)) { return $Obj[$Name] }
        return $null
    }
    $p = $Obj.PSObject.Properties[$Name]
    if ($p) { return $p.Value }
    return $null
}

function Write-WdRefusal {
    # 拒绝记录：写证据目录后直接退出。除任务配置核验外**不发起任何网络动作**（Flag: NetworkActions）。
    param([int]$Code, [string]$Label, [string]$Reason, [hashtable]$Extra, [int]$NetworkActions = 0)
    # BLOCKED-01：拒绝记录**也是 summary.json**，因此同样要给出**独立的**运行状态观测字段，
    # 与配置门判据分开呈现 —— 现场不得把 diagnostics.point_count 当成"当前任务点数"。
    # 任务配置核验之前的拒绝（G1/G2/注入守卫/缺授权）没有 actual，相关字段一律 $null。
    $refCreds = if ($Extra -and $Extra.ContainsKey('credentials')) { $Extra['credentials'] } else { $null }
    $refTaskItem = Get-WdProp (Get-WdProp $refCreds 'items') 'task_config'
    $refTaskActual = Get-WdProp $refTaskItem 'actual'
    $rec = [ordered]@{
        mode            = $Mode
        pid             = $PID
        refused         = $Label
        exit_code       = $Code
        reason          = $Reason
        note            = '授权边界拒绝，非设备故障（未发出 Start / 未进入 FIRED）'
        base_url        = $BaseUrl
        endpoint        = $EndpointId
        t_local         = (Get-Date).ToString('o')
        authorized      = [bool]$Authorized
        authorized_by   = $AuthorizedBy
        credentials     = [ordered]@{
            authz_ticket      = $AuthzTicket
            g6_signoff        = $G6Signoff
            g6_signed_at      = $G6SignedAt
            g7_deploy_version = $G7DeployVersion
            g7_binary_path    = $G7BinaryPath
            expected_manifest_sha256 = $ExpectedManifestSha256
            expected_revision = $ExpectedTaskRevision
            # 语义：获批任务配置的点数（BLOCKED-01 起；不是 diagnostics.point_count）。
            expected_task_point_count = $ExpectedPointCount
            approved_task_config = $ApprovedTaskConfig
            expected_tasks_fingerprint = $ExpectedTaskFingerprint
            expected_approved_task_config_sha256 = $ExpectedApprovedTaskConfigSha256
        }
        network_actions = $NetworkActions
        # BLOCKED-01：判据 vs 观测值（两者语义不同，不得互相顶替）
        task_config_observation = [ordered]@{
            expected_task_point_count        = $ExpectedPointCount
            task_point_count_live            = (Get-WdProp $refTaskActual 'task_point_count')
            task_count_live                  = (Get-WdProp $refTaskActual 'task_count')
            diagnostics_point_count_observed = (Get-WdProp $refTaskActual 'diagnostics_point_count')
            diagnostics_point_count_role     = 'observed-only：运行状态残留值（持久化 point_registry 行数，core-api:1499 ← config-store:2459-2473），非当前任务点数，不参与配置门判定'
            task_config_state                = (Get-WdProp $refTaskItem 'state')
        }
        extra           = $Extra
    }
    [System.IO.File]::WriteAllText($summaryPath, ($rec | ConvertTo-Json -Depth 8), [System.Text.UTF8Encoding]::new($false))
    Add-WdLog $ctx 'ERROR' "拒绝执行：$Label（退出码 $Code）—— $Reason"
    Write-Host ''
    Write-Host '================ stop-watchdog 拒绝执行（授权边界，非设备故障）================'
    Write-Host ("模式            : {0}" -f $Mode)
    Write-Host ("拒绝原因        : {0}" -f $Label)
    Write-Host ("说明            : {0}" -f $Reason)
    Write-Host ("ExitCode        : {0}（40 = 授权边界拒绝，不是 Stop 失败/设备故障）" -f $Code)
    Write-Host ("已发出 Start    : 否（拒绝发生在 FIRED 之前）")
    Write-Host ("网络动作        : {0} 次{1}" -f $NetworkActions, $(if ($NetworkActions -gt 0) { '（仅任务配置核验的只读 GET）' } else { '（未建立任何连接）' }))
    if ($script:TestInjectionActive -and $script:TestInjectionActive.Count -gt 0) {
        Write-Host ("测试注入参数    : {0}（离线测试专用；被本守卫拒绝时不允许用于真实执行）" -f ($script:TestInjectionActive -join ' '))
    }
    Write-Host ("证据目录        : {0}" -f $OutDir)
    Write-Host '==========================================================================='
    exit $Code
}

$urlInfo = Get-WdUrlInfo -Url $ctx.baseUrl
$boundary = [ordered]@{
    host       = $urlInfo.host
    port       = $urlInfo.port
    loopback   = $urlInfo.loopback
    allowed    = $false
    decided_by = ''
    reason     = ''
}

if ($Mode -eq 'arm') {
    if (-not $urlInfo.ok) {
        $boundary.reason = $urlInfo.reason
    }
    elseif (-not $urlInfo.loopback) {
        $boundary.reason = "$($urlInfo.reason)（ARMED 只允许明确指定的离线模拟端点，不得指向真实设备）"
    }
    elseif ($urlInfo.port -eq $script:PRODUCTION_CORE_PORT) {
        $boundary.reason = "端口 $($script:PRODUCTION_CORE_PORT) 是生产 mesad（ARMED 永远不碰生产实例，请改用离线桩端口）"
    }
    else {
        $boundary.allowed = $true
        $boundary.decided_by = 'arm：OFFLINE_ONLY(回环 && 端口≠8132)'
        $boundary.reason = "$($urlInfo.host):$($urlInfo.port) 回环离线端点"
    }
    if (-not $boundary.allowed) {
        Write-WdRefusal -Code 40 -Label 'REFUSED_NON_OFFLINE' -Reason $boundary.reason -Extra @{ host = $urlInfo.host; port = $urlInfo.port; loopback = $urlInfo.loopback }
    }
}
else {
    # fire：地址放行与否取决于凭证校验结果（在下一段判定），此处只记录状态。
    $boundary.reason = 'fire：等待四项凭证校验；通过后放行任意 host（含生产 mesad 8132 与现场地址）'
}
Add-WdLog $ctx 'INFO' ("授权边界：mode={0} host={1}:{2} loopback={3} allowed={4}；{5}" -f $Mode, $urlInfo.host, $urlInfo.port, $urlInfo.loopback, $boundary.allowed, $boundary.reason)
#endregion

#region ── B1：测试注入参数的离线边界（统一守卫；先于任何网络动作）────────────────────
# 复审阻塞 B1：-TestHostProbeError / -TestAbortFlow 是**离线测试专用**注入开关，旧版只在
# 各自使用点（HostChecks 的 ④⑤、协调者主循环的注入中止）读取，**没有统一保护**；而 fire 模式
# 放行任意 host。于是"把测试参数带进真实执行命令"会在 Start 之后**故意**中止工作流（WORKER_ABORT）
# 或把本机核对伪造成"无法确定"——那不是设备故障，是我们自己制造的异常，在生产 mesad（8132）
# 或现场机床上不可接受。
#
# 冻结规则：注入参数只允许用于**明确的离线回环端点**（loopback 且端口≠8132）。
# 违反 ⇒ 在任何网络动作之前 exit 40（REFUSED_TEST_INJECTION_ONLINE），network_actions = 0。
# 实现位置刻意放在"参数解析之后、任何 HTTP 之前"：本 region 位于 arm 探活（GET 快照）与
# fire 凭证核验（只读 GET）之前，因此拒绝时**连一条 TCP 连接都不会建立**（M13 断言网络动作数=0）。
$script:TestInjectionActive = @()
if (-not [string]::IsNullOrWhiteSpace($TestHostProbeError)) { $script:TestInjectionActive += "-TestHostProbeError=$TestHostProbeError" }
if (-not [string]::IsNullOrWhiteSpace($TestAbortFlow)) { $script:TestInjectionActive += "-TestAbortFlow=$TestAbortFlow" }
# 说明：-TestAbortAfterSec 单独出现**不构成**注入（它只在 -TestAbortFlow 生效时才被读取），
# 故不计入；这与"注入必须显式命名被注入的对象"的语义一致。

function Test-WdInjectionGuard {
    # 统一的注入边界判定（唯一判定点，不在各使用点零散判断）。
    # 返回 [ordered]@{ active; allowed; reason; decided_by; ... }；allowed=false 时调用方必须 exit 40。
    param($UrlInfo, [string[]]$InjectionParams)
    $active = (@($InjectionParams).Count -gt 0)
    $g = [ordered]@{
        active          = $active
        params          = @($InjectionParams)
        allowed         = $true
        host            = $UrlInfo.host
        port            = $UrlInfo.port
        loopback        = [bool]$UrlInfo.loopback
        production_port = $script:PRODUCTION_CORE_PORT
        network_actions = 0
        reason          = '未携带测试注入参数（正常执行路径，不受本守卫约束）'
        decided_by      = 'injection-guard：不适用'
    }
    if (-not $active) { return $g }
    if (-not $UrlInfo.ok) {
        $g.allowed = $false
        $g.decided_by = 'injection-guard：注入参数要求合法 http/https 回环 BaseUrl'
        $g.reason = "携带测试注入参数（$($InjectionParams -join ', ')），但 BaseUrl 不合法：$($UrlInfo.reason)"
        return $g
    }
    if (-not $UrlInfo.loopback) {
        $g.allowed = $false
        $g.decided_by = 'injection-guard：OFFLINE_ONLY(loopback)'
        $g.reason = "携带测试注入参数（$($InjectionParams -join ', ')）却指向非回环地址 $($UrlInfo.host)：测试注入只允许用于明确的离线回环端点"
        return $g
    }
    if ($UrlInfo.port -eq $script:PRODUCTION_CORE_PORT) {
        $g.allowed = $false
        $g.decided_by = 'injection-guard：拒绝生产端口'
        $g.reason = "携带测试注入参数（$($InjectionParams -join ', ')）却指向生产端口 $($script:PRODUCTION_CORE_PORT)（生产 mesad）：测试注入只允许用于离线桩端口"
        return $g
    }
    $g.decided_by = 'injection-guard：OFFLINE_ONLY(loopback && 端口≠8132) 通过'
    $g.reason = "携带测试注入参数（$($InjectionParams -join ', ')）指向回环离线端点 $($UrlInfo.host):$($UrlInfo.port) ⇒ 允许（仅离线测试）"
    return $g
}

$injectionGuard = Test-WdInjectionGuard -UrlInfo $urlInfo -InjectionParams $script:TestInjectionActive
Add-WdLog $ctx $(if ($injectionGuard.allowed) { 'INFO' } else { 'ERROR' }) ("测试注入边界：{0}（{1}）" -f $injectionGuard.reason, $injectionGuard.decided_by)
if (-not $injectionGuard.allowed) {
    Write-WdRefusal -Code 40 -Label 'REFUSED_TEST_INJECTION_ONLINE' -Reason $injectionGuard.reason -Extra @{
        injection_guard = $injectionGuard
        hint            = '测试注入参数（-TestHostProbeError / -TestAbortFlow）只能配合 127.0.0.1 的非 8132 端口（tests/watchdog/ 的假端点桩）；真实执行必须去掉这些参数'
    } -NetworkActions 0
}
#endregion

#region ── 四项凭证：**校验**而不是"接收"（G6 签署 / G7 部署版本 / 任务配置 / 显式授权）──
# 约定：每项 state ∈ verified / invalid / missing / unverified / not_checked
#   verified   = 校验通过（G7 与仓库 HEAD 或部署二进制指纹一致；任务配置与实际只读 GET 一致）
#   invalid    = 有值但校验不通过（版本不一致、时间不可解析、revision/点数不匹配）
#   missing    = 未提供
#   unverified = 无法校验（无 git/非 git 工作树、端点读不到等）→ **按未通过处理**
function Get-WdRepoRoot {
    $self = $PSCommandPath
    if ([string]::IsNullOrWhiteSpace($self)) { $self = $MyInvocation.MyCommand.Path }
    return (Split-Path -Parent (Split-Path -Parent $self))
}

function New-WdCredItem {
    # 凭证项统一形状。label 是可选的**拒绝标签**：凭证校验给出"更精确"的拒绝理由时使用
    # （G1 ⇒ REFUSED_MANIFEST_INTEGRITY / G2 ⇒ REFUSED_TASKCONFIG_INTEGRITY），
    # 让现场与测试能从 summary.refused 直接看出是哪一类完整性问题，而不是笼统的 CREDENTIAL_INVALID。
    param([string]$Id, [string]$State, [string]$Detail, $Provided, $Actual, [string]$Label = '')
    return [ordered]@{ id = $Id; state = $State; detail = $Detail; provided = $Provided; actual = $Actual; label = $Label }
}

function Test-WdCredG6 {
    # G6 现场签署：签署标识/编号 + 签署时间；非空且时间可解析、不在未来。
    param([string]$Signoff, [string]$SignedAt)
    $provided = [ordered]@{ signoff = $Signoff; signed_at = $SignedAt }
    if ([string]::IsNullOrWhiteSpace($Signoff)) { return (New-WdCredItem 'g6_signoff' 'missing' '未提供 -G6Signoff（现场签署人/签署编号）' $provided $null) }
    if ([string]::IsNullOrWhiteSpace($SignedAt)) { return (New-WdCredItem 'g6_signoff' 'missing' '未提供 -G6SignedAt（签署时间，ISO 8601）' $provided $null) }
    $dt = [datetimeoffset]::Now
    if (-not [datetimeoffset]::TryParse($SignedAt, [ref]$dt)) {
        return (New-WdCredItem 'g6_signoff' 'invalid' "签署时间无法解析：$SignedAt（应形如 2026-10-24T09:00:00+08:00）" $provided $null)
    }
    if ($dt -gt [datetimeoffset]::Now.AddMinutes(5)) {
        return (New-WdCredItem 'g6_signoff' 'invalid' "签署时间在未来：$($dt.ToString('o'))（疑似填错）" $provided $null)
    }
    return (New-WdCredItem 'g6_signoff' 'verified' "签署=$Signoff；签署时间=$($dt.ToString('o'))" $provided ([ordered]@{ parsed = $dt.ToString('o') }))
}

function Get-WdFileSha256 {
    # 文件 SHA-256（小写十六进制）。部署二进制身份的唯一可计算依据（B2）。
    param([string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function ConvertTo-WdDigestHex {
    # 把 "sha256:<64 hex>" 或裸 "<64 hex>" 规范化为小写 64 位十六进制；格式非法返回 $null。
    # 之所以两种都接受：签署包里常写成 sha256:<hex>（与 -G7DeployVersion 的形式一致），
    # 但人工转录时偶尔只抄 hex；两种写法代表同一个摘要，不构成放宽（格式仍然严格 64 位十六进制）。
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return $null }
    $v = $Value.Trim()
    if ($v -notmatch '^(?i)(?:sha256:)?([0-9a-f]{64})$') { return $null }
    return $Matches[1].ToLowerInvariant()
}

function Read-WdRawFileBytes {
    # 读取文件**原始字节**并计算 SHA-256（G1/G2 的唯一实现）。
    #
    # 为什么必须是"原始字节"而不是"解析后的对象"：
    #   1) 若先解析再比对，比对的对象可能已经不是"被签署的那个文件内容"（解析器会规范化/丢字段），
    #      而且"先解析"本身就给了"解析结果与摘要不同源"的绕过空间；
    #   2) 一次 ReadAllBytes 之后，**同一份字节**既用于算摘要也交给后续解析（$r.bytes），
    #      因此比对与解析读到的必然是同一份内容，不存在 TOCTOU（比对后文件被换掉再解析）。
    # 调用方必须先调用本函数、比对成功之后才允许解析 $r.bytes。
    param([string]$Path)
    $r = [ordered]@{ ok = $false; path = $Path; bytes = $null; size = 0; sha256 = $null; note = '' }
    if ([string]::IsNullOrWhiteSpace($Path)) { $r.note = '路径为空'; return $r }
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { $r.note = "文件不存在：$Path"; return $r }
    try {
        $r.path = (Resolve-Path -LiteralPath $Path).Path
        $bytes = [System.IO.File]::ReadAllBytes($r.path)
    }
    catch {
        $r.note = "读取原始字节失败：$($_.Exception.Message)"
        return $r
    }
    $r.bytes = $bytes
    $r.size = $bytes.Length
    $r.sha256 = [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
    $r.ok = $true
    return $r
}

function ConvertFrom-WdJsonBytes {
    # 把**已校验过摘要的**原始字节解析成 JSON 对象（与 Read-WdRawFileBytes 配对使用）。
    # 单独成函数是为了让"摘要 → 解析"的先后顺序在代码里一眼可见：解析只接受字节，不接受路径。
    param([byte[]]$Bytes)
    $out = [ordered]@{ ok = $false; doc = $null; note = '' }
    try {
        $text = [System.Text.Encoding]::UTF8.GetString($Bytes)
        $out.doc = $text | ConvertFrom-Json
        $out.ok = $true
    }
    catch {
        $out.note = $_.Exception.Message
    }
    return $out
}

function Get-WdGitAux {
    # git HEAD / describe 在 B2 之后**只是辅助信息**：它们只能证明"输入值与仓库版本匹配"，
    # **不能**独立证明"实际运行的 mesad 二进制就是这个版本构建的"（复审阻塞 B2）。
    # 因此这里只采集、只记录（写进 summary/t0.stamp 供追溯），**绝不参与 G7 通过判定**。
    param([string]$Root)
    $aux = [ordered]@{
        role          = 'auxiliary-only（B2：仅记录，不作为 G7 通过依据）'
        repo_root     = $Root
        git_available = $false
        git_head      = $null
        git_describe  = $null
    }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { return $aux }
    try {
        $h = (& git -C $Root rev-parse HEAD 2>$null | Select-Object -First 1)
        if (-not [string]::IsNullOrWhiteSpace($h)) {
            $aux.git_available = $true
            $aux.git_head = ([string]$h).Trim()
            $d = (& git -C $Root describe --tags --always --dirty 2>$null | Select-Object -First 1)
            if (-not [string]::IsNullOrWhiteSpace($d)) { $aux.git_describe = ([string]$d).Trim() }
        }
    }
    catch { }
    return $aux
}

function Resolve-WdDeployBinary {
    # 定位"实际部署的二进制"。显式 -G7BinaryPath 优先且**不回退**：显式给了却不存在 ⇒ $null
    # （fail-closed —— 绝不悄悄换成 target/ 下的另一个二进制来凑出"通过"）。
    # 未显式指定时才按固定顺序探测默认产物路径（并如实记录最终比对的是哪一个文件）。
    param([string]$Explicit)
    if (-not [string]::IsNullOrWhiteSpace($Explicit)) {
        $p = $Explicit.Trim()
        if (Test-Path -LiteralPath $p -PathType Leaf) { return (Resolve-Path -LiteralPath $p).Path }
        return $null
    }
    $root = Get-WdRepoRoot
    foreach ($rel in @('target/release/mesad.exe', 'target/release/mesad', 'target/debug/mesad.exe', 'target/debug/mesad')) {
        $bin = Join-Path $root $rel
        if (Test-Path -LiteralPath $bin -PathType Leaf) { return (Resolve-Path -LiteralPath $bin).Path }
    }
    return $null
}

function Get-WdManifestDigestFromDoc {
    # 从**已解析**的受信构建清单对象里取期望二进制摘要；缺字段/格式错一律 ok=false（fail-closed）。
    # 注意：本函数只接受**已解析对象**，调用方必须先用 Read-WdRawFileBytes 校验清单**自身**的摘要
    # （G1），再用 ConvertFrom-WdJsonBytes 解析同一份字节 —— 顺序不可颠倒。
    # 支持两种形态：
    #   {"binary": "<相对/绝对路径>", "sha256": "<64 hex>"}                  —— 单产物
    #   {"artifacts": {"mesad.exe": "<64 hex>", "mesad": "<64 hex>"}}         —— 多产物（按文件名取）
    param($Doc, [string]$BinaryName)
    $digest = $null
    $binRel = $null
    $note = ''
    if ($Doc.PSObject.Properties['binary'] -and -not [string]::IsNullOrWhiteSpace([string]$Doc.binary)) { $binRel = [string]$Doc.binary }
    if ($Doc.PSObject.Properties['sha256'] -and -not [string]::IsNullOrWhiteSpace([string]$Doc.sha256)) {
        $digest = ([string]$Doc.sha256).Trim().ToLowerInvariant()
    }
    elseif ($Doc.PSObject.Properties['artifacts'] -and $Doc.artifacts) {
        $props = @($Doc.artifacts.PSObject.Properties)
        foreach ($pr in $props) {
            if ($BinaryName -and $pr.Name -eq $BinaryName) { $digest = ([string]$pr.Value).Trim().ToLowerInvariant(); break }
        }
        if (-not $digest) {
            # 未按名命中：清单只有一个产物时允许取它（摘要格式仍需合法）；多产物则必须按名命中。
            if ($props.Count -eq 1) { $digest = ([string]$props[0].Value).Trim().ToLowerInvariant() }
            else { $note = "受信清单 artifacts 未包含 $BinaryName（共 $($props.Count) 项）" }
        }
    }
    if (-not $digest) {
        if (-not $note) { $note = '受信清单缺少 sha256 / artifacts 摘要字段' }
        return [ordered]@{ ok = $false; sha256 = $null; binary = $binRel; note = $note }
    }
    if ($digest -notmatch '^[0-9a-f]{64}$') {
        return [ordered]@{ ok = $false; sha256 = $null; binary = $binRel; note = "受信清单摘要不是 64 位十六进制：$digest" }
    }
    return [ordered]@{ ok = $true; sha256 = $digest; binary = $binRel; note = '' }
}

function Test-WdCredG7 {
    # G7 部署版本确认：**只**接受能独立证明"实际部署二进制身份"的两种形式
    #   ① sha256:<64 hex>   —— 与指定部署二进制（-G7BinaryPath；缺省探测 target/{release,debug}/mesad[.exe]）
    #                          的**实算**摘要比对；
    #   ② manifest:<path>   —— 从受信构建清单读取期望摘要，再与部署二进制实算摘要比对。
    #                          **G1 之后**：清单**自身**还必须与 -ExpectedManifestSha256（来自签署包）比对，
    #                          且该比对发生在**解析清单内容之前**、对**原始文件字节**进行。
    # git HEAD / describe **只作辅助信息**记录，绝不作为通过依据（旧实现拿它当通过依据 = 阻塞 B2）。
    # 两者都不可用（未给摘要/清单、清单不可读或格式错、定位不到二进制、摘要算不出）
    #   ⇒ unverified ⇒ 拒绝进入 FIRED（fail-closed），**不得**降级为"用 git HEAD 代替"。
    param([string]$Provided, [string]$BinaryPath = '', [string]$ExpectedManifestSha256 = '')
    $aux = Get-WdGitAux -Root (Get-WdRepoRoot)
    if ([string]::IsNullOrWhiteSpace($Provided)) {
        return (New-WdCredItem 'g7_deploy_version' 'missing' '未提供 -G7DeployVersion（必须 sha256:<64 hex> 或 manifest:<path>；git HEAD/describe 只能作辅助信息，不作为通过依据）' $null $aux)
    }
    $p = $Provided.Trim()
    $want = $null
    $source = ''
    $manifestInfo = $null
    if ($p -match '^(?i)sha256:([0-9a-f]{64})$') {
        $want = $Matches[1].ToLowerInvariant()
        $source = 'sha256'
    }
    elseif ($p -match '^(?i)manifest:(.+)$') {
        $source = 'manifest'
        $mPath = $Matches[1].Trim()
        if (-not (Test-Path -LiteralPath $mPath -PathType Leaf)) {
            $actual = [ordered]@{ source = 'manifest'; manifest = [ordered]@{ path = $mPath; ok = $false; note = '读不到' }; git = $aux }
            return (New-WdCredItem 'g7_deploy_version' 'unverified' "受信清单读不到：$mPath → 无法获得期望摘要（按未通过处理；不用 git HEAD 替代）" $p $actual)
        }

        # ── G1：清单**自身**的完整性（必须先于解析，且针对原始文件字节）────────────────
        # 复审阻塞：只比"清单里的二进制摘要 ↔ 实际二进制"时，**清单与二进制一起被替换**仍然互相匹配；
        # 清单自身必须有一个来自**签署包**的独立摘要，否则"校验通过"只证明了"两份被换过的东西彼此一致"。
        $wantManifest = ConvertTo-WdDigestHex -Value $ExpectedManifestSha256
        if ([string]::IsNullOrWhiteSpace($ExpectedManifestSha256)) {
            $actual = [ordered]@{
                source   = 'manifest'
                manifest = [ordered]@{ path = $mPath; ok = $false; note = '未提供 -ExpectedManifestSha256' }
                integrity = [ordered]@{ expected = $null; actual = $null; ok = $false; verified_before_parse = $false }
                git      = $aux
            }
            return (New-WdCredItem 'g7_deploy_version' 'missing' '未提供 -ExpectedManifestSha256（受信构建清单自身的 SHA-256，必须取自签署的 G6/G7 执行包）：缺它时"清单+二进制一起被替换"仍会互相匹配 → 拒绝' $p $actual -Label 'REFUSED_MANIFEST_INTEGRITY')
        }
        if (-not $wantManifest) {
            $actual = [ordered]@{
                source    = 'manifest'
                manifest  = [ordered]@{ path = $mPath; ok = $false; note = 'ExpectedManifestSha256 格式非法' }
                integrity = [ordered]@{ expected = $ExpectedManifestSha256; actual = $null; ok = $false; verified_before_parse = $false }
                git       = $aux
            }
            return (New-WdCredItem 'g7_deploy_version' 'invalid' "ExpectedManifestSha256 格式非法：$ExpectedManifestSha256（应形如 sha256:<64 hex>）" $p $actual -Label 'REFUSED_MANIFEST_INTEGRITY')
        }
        $rawManifest = Read-WdRawFileBytes -Path $mPath
        if (-not $rawManifest.ok) {
            $actual = [ordered]@{
                source    = 'manifest'
                manifest  = [ordered]@{ path = $mPath; ok = $false; note = $rawManifest.note }
                integrity = [ordered]@{ expected = $wantManifest; actual = $null; ok = $false; verified_before_parse = $false }
                git       = $aux
            }
            return (New-WdCredItem 'g7_deploy_version' 'unverified' ("受信清单原始字节读不到（{0}）→ 无法核对清单自身摘要（按未通过处理）" -f $rawManifest.note) $p $actual)
        }
        $mIntegrity = [ordered]@{
            role                 = 'manifest-integrity（G1：清单自身与签署包批准值比对）'
            path                 = $rawManifest.path
            size                 = $rawManifest.size
            expected             = $wantManifest
            actual               = $rawManifest.sha256
            ok                   = ($rawManifest.sha256 -eq $wantManifest)
            verified_before_parse = $true
            compared_at          = (Get-Date).ToString('o')
        }
        if (-not $mIntegrity.ok) {
            $actual = [ordered]@{
                source    = 'manifest'
                manifest  = [ordered]@{ path = $rawManifest.path; ok = $false; sha256_self = $rawManifest.sha256; integrity = $mIntegrity }
                git       = $aux
            }
            return (New-WdCredItem 'g7_deploy_version' 'invalid' ("受信清单原始文件的 SHA-256（{0}）与签署包批准值（{1}）不一致：清单内容未获独立批准（清单与二进制一起被替换时二者仍互相匹配）→ 拒绝" -f $rawManifest.sha256, $wantManifest) $p $actual -Label 'REFUSED_MANIFEST_INTEGRITY')
        }

        # 完整性通过后才解析：解析的正是**校验过的那份字节**（ConvertFrom-WdJsonBytes 只接受字节）。
        $parsed = ConvertFrom-WdJsonBytes -Bytes $rawManifest.bytes
        if (-not $parsed.ok) {
            $actual = [ordered]@{
                source    = 'manifest'
                manifest  = [ordered]@{ path = $rawManifest.path; ok = $false; sha256_self = $rawManifest.sha256; integrity = $mIntegrity; note = '不是合法 JSON' }
                git       = $aux
            }
            return (New-WdCredItem 'g7_deploy_version' 'unverified' "受信清单不是合法 JSON：$($parsed.note)（按未通过处理）" $p $actual)
        }
        $mPath = $rawManifest.path
        $binName = if (-not [string]::IsNullOrWhiteSpace($BinaryPath)) { Split-Path -Leaf $BinaryPath } else { '' }
        $manifestInfo = Get-WdManifestDigestFromDoc -Doc $parsed.doc -BinaryName $binName
        $manifestInfo['path'] = $mPath
        $manifestInfo['sha256_self'] = $rawManifest.sha256
        $manifestInfo['integrity'] = $mIntegrity
        if (-not $manifestInfo.ok) {
            $actual = [ordered]@{ source = 'manifest'; manifest = $manifestInfo; git = $aux }
            return (New-WdCredItem 'g7_deploy_version' 'unverified' ("受信清单不可用（{0}）→ 无法获得期望摘要（按未通过处理）" -f $manifestInfo.note) $p $actual)
        }
        $want = [string]$manifestInfo.sha256
        # 清单内声明的 binary（相对清单目录解析）作为部署二进制，除非显式给了 -G7BinaryPath。
        if ([string]::IsNullOrWhiteSpace($BinaryPath) -and -not [string]::IsNullOrWhiteSpace([string]$manifestInfo.binary)) {
            $cand = [string]$manifestInfo.binary
            if (-not [System.IO.Path]::IsPathRooted($cand)) { $cand = Join-Path (Split-Path -Parent $mPath) $cand }
            $BinaryPath = $cand
        }
    }
    else {
        # 形如 commit SHA / git describe：只能证明"与仓库版本匹配"，不能证明部署物身份 → fail-closed。
        $actual = [ordered]@{ source = 'rejected-form'; provided = $p; git = $aux }
        return (New-WdCredItem 'g7_deploy_version' 'unverified' "提供的值 `"$p`" 不是 sha256:<64 hex> / manifest:<path>：git 版本标识不能独立证明部署二进制身份（B2）→ 按未通过处理，不做降级放行" $p $actual)
    }
    # G1 的反向约束：给了"清单自身摘要"却不走清单形式 ⇒ 拒绝（不静默忽略一个已批准的摘要）。
    if ($source -ne 'manifest' -and -not [string]::IsNullOrWhiteSpace($ExpectedManifestSha256)) {
        $actual = [ordered]@{ source = $source; provided = $p; expected_manifest_sha256 = $ExpectedManifestSha256; git = $aux }
        return (New-WdCredItem 'g7_deploy_version' 'invalid' "已提供 -ExpectedManifestSha256，但 -G7DeployVersion 不是 manifest:<path>（该批准值无处可用）：不允许静默忽略签署包里的清单摘要 → 拒绝" $p $actual -Label 'REFUSED_MANIFEST_INTEGRITY')
    }
    $binary = Resolve-WdDeployBinary -Explicit $BinaryPath
    if (-not $binary) {
        $where = if ([string]::IsNullOrWhiteSpace($BinaryPath)) {
            '未给 -G7BinaryPath，且 target/{release,debug}/mesad[.exe] 均不存在'
        }
        else { "指定的 -G7BinaryPath 不存在：$BinaryPath（显式指定不回退到其它路径）" }
        $actual = [ordered]@{ source = $source; binary = $null; binary_path_param = $BinaryPath; manifest = $manifestInfo; git = $aux }
        return (New-WdCredItem 'g7_deploy_version' 'unverified' "无法定位部署二进制（$where）→ 无法实算比对（按未通过处理）" $p $actual)
    }
    try { $h = Get-WdFileSha256 -Path $binary }
    catch {
        $actual = [ordered]@{ source = $source; binary = $binary; err = $_.Exception.Message; git = $aux }
        return (New-WdCredItem 'g7_deploy_version' 'unverified' "部署二进制无法计算摘要（$binary）：$($_.Exception.Message)（按未通过处理）" $p $actual)
    }
    $actual = [ordered]@{
        source      = $source
        binary      = $binary
        sha256      = $h
        expected    = $want
        manifest    = $manifestInfo
        git         = $aux
        compared_at = (Get-Date).ToString('o')
    }
    if ($h -eq $want) {
        return (New-WdCredItem 'g7_deploy_version' 'verified' "部署二进制实算摘要与期望一致（source=$source；binary=$binary；sha256=$h）" $p $actual)
    }
    return (New-WdCredItem 'g7_deploy_version' 'invalid' "部署二进制实算摘要与期望不一致（source=$source；binary=$binary；实际 $h，期望 $want）" $p $actual)
}

# ── B3：任务配置的规范化 / 指纹 / 完整点位比对 ────────────────────────────────────
function ConvertTo-WdCanonicalNode {
    # 规范化节点（供指纹计算）：对象键**递归排序**，数组保持原序，标量原样。
    # 目的：同一份配置的不同书写顺序（键顺序）得到同一指纹，同时**不抹平任何值差异**
    # （值差异必须体现为指纹差异，否则指纹就失去"证明配置未变"的作用）。
    # 输出与 python `json.dumps(sort_keys=True, separators=(',',':'))` 逐字节一致
    # （tests/watchdog 用 python 独立实现交叉核对，见 run-watchdog-scenarios.ps1 的指纹用例）。
    param($Node)
    if ($null -eq $Node) { return $null }
    if ($Node -is [string] -or $Node -is [bool] -or $Node -is [int] -or $Node -is [long] -or $Node -is [double] -or $Node -is [decimal]) { return $Node }
    if ($Node -is [System.Collections.IDictionary]) {
        $o = [ordered]@{}
        foreach ($k in (@($Node.Keys) | Sort-Object -Property { [string]$_ })) { $o[[string]$k] = ConvertTo-WdCanonicalNode -Node $Node[$k] }
        return $o
    }
    if ($Node -is [System.Collections.IEnumerable]) {
        $arr = [System.Collections.Generic.List[object]]::new()
        foreach ($it in $Node) { $arr.Add((ConvertTo-WdCanonicalNode -Node $it)) }
        return , ($arr.ToArray())
    }
    $o = [ordered]@{}
    foreach ($p in (@($Node.PSObject.Properties) | Sort-Object -Property Name)) { $o[$p.Name] = ConvertTo-WdCanonicalNode -Node $p.Value }
    return $o
}

function Get-WdTasksFingerprint {
    # 任务配置指纹 = SHA-256(规范化 tasks 数组)，输出 "sha256:<64 hex>"。
    # 规范化 = 每个 task 键递归排序 + **按 task.id 排序**（任务集合无序、id 唯一；同 id 用规范 JSON
    # 兜底以保证确定性）+ 紧凑 JSON（UTF-8、无空白）。
    # 这是 B3 的"批准版本一致性"依据：revision 与点数相同**不足以**证明配置一致。
    param($Tasks)
    $items = @()
    foreach ($t in @($Tasks)) {
        if ($null -eq $t) { continue }
        $id = ''
        if ($t.PSObject.Properties['id']) { $id = [string]$t.id }
        $canon = ConvertTo-WdCanonicalNode -Node $t
        $items += [pscustomobject]@{ id = $id; canon = $canon; json = (ConvertTo-Json -InputObject $canon -Compress -Depth 100) }
    }
    $sorted = @($items | Sort-Object -Property id, json)
    $payload = ConvertTo-Json -InputObject @($sorted | ForEach-Object { $_.canon }) -Compress -Depth 100
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
    $hex = [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
    return [ordered]@{ fingerprint = "sha256:$hex"; payload = $payload; count = $sorted.Count }
}

function Read-WdApprovedTaskConfig {
    # 读"获批任务配置"（**本地文件，不涉及网络**）。缺字段/格式错 ⇒ ok=false（fail-closed）。
    # 文件形态（tests/watchdog 的桩与场景驱动共用同一份，字段名与真实 schema 对齐）：
    #   {
    #     "schema": "mesa.stop-watchdog.approved-task-config.v1",
    #     "endpoint_id": "...", "revision": "rev13",
    #     "connection": {"host","port","local_tsap","remote_tsap","pdu_length"},
    #     "tasks": [ {"id","schedule","binding":{"kind","config":{"points":[...]}}} ]
    #   }
    # G2 之后本函数**优先接受字节**（-Bytes）：调用方必须先用 Read-WdRawFileBytes 校验文件自身摘要，
    # 再把**同一份字节**交进来解析。只有 -Bytes 为空时才回退到按路径读（仅供不需要 G2 的旧调用点，
    # 当前脚本内已无此类调用点）。
    param([string]$Path, [byte[]]$Bytes = $null, [string]$FileSha256 = '')
    $r = [ordered]@{ ok = $false; note = ''; path = $Path; schema = $null; endpoint_id = $null; revision = $null; connection = $null; tasks = $null; tasks_fingerprint = $null; tasks_payload = $null; file_sha256 = $FileSha256; parsed_from = '' }
    if ($null -ne $Bytes -and $Bytes.Length -gt 0) {
        $r.parsed_from = 'verified-bytes'
        $parsed = ConvertFrom-WdJsonBytes -Bytes $Bytes
        if (-not $parsed.ok) { $r.note = "获批任务配置不是合法 JSON：$($parsed.note)"; return $r }
        $doc = $parsed.doc
    }
    else {
        if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { $r.note = "获批任务配置读不到：$Path"; return $r }
        $r.parsed_from = 'path（未做文件自身摘要校验的旧路径）'
        try { $doc = Get-Content -LiteralPath $r.path -Raw | ConvertFrom-Json }
        catch { $r.note = "获批任务配置不是合法 JSON：$($_.Exception.Message)"; return $r }
    }
    if (-not [string]::IsNullOrWhiteSpace($Path) -and (Test-Path -LiteralPath $Path -PathType Leaf)) {
        $r.path = (Resolve-Path -LiteralPath $Path).Path
    }
    if ($doc.PSObject.Properties['schema']) { $r.schema = [string]$doc.schema }
    if ($doc.PSObject.Properties['endpoint_id']) { $r.endpoint_id = [string]$doc.endpoint_id }
    if ($doc.PSObject.Properties['revision']) { $r.revision = [string]$doc.revision }
    if ($doc.PSObject.Properties['connection']) { $r.connection = $doc.connection }
    if ($doc.PSObject.Properties['tasks']) { $r.tasks = @($doc.tasks) }
    if ($null -eq $r.connection) { $r.note = '获批任务配置缺少 connection（host / port / local_tsap / remote_tsap / pdu_length）'; return $r }
    if ($null -eq $r.tasks) { $r.note = '获批任务配置缺少 tasks 数组（完整点位 Key 与参数）'; return $r }
    $fp = Get-WdTasksFingerprint -Tasks $r.tasks
    $r.tasks_fingerprint = $fp.fingerprint
    $r.tasks_payload = $fp.payload
    $r.ok = $true
    return $r
}

function Test-WdApprovedFileIntegrity {
    # G2：获批任务配置**文件自身**的完整性 —— 必须与签署包里的独立批准值比对。
    #
    # 为什么不能只靠 -ExpectedTaskFingerprint：指纹 = SHA-256(规范化 **tasks 数组**)，
    # **connection（host/port/TSAP/PDU）不在其中**；改动 connection 时指纹一字不变，
    # 于是"指纹相同"并不能证明"这份文件就是签署时批准的那份"。只有文件原始字节的摘要能证明。
    #
    # 顺序要求：**先对原始字节算摘要并比对，比对通过后才允许解析**（本函数只返回字节，不解析）。
    # 返回 state ∈ verified / missing / invalid / unverified，与凭证项的状态语义一致。
    param([string]$Path, [string]$ExpectedSha256)
    $r = [ordered]@{
        state                  = 'unverified'
        detail                 = ''
        path                   = $Path
        size                   = 0
        expected               = $null
        actual                 = $null
        ok                     = $false
        verified_before_parse  = $false
        compared_at            = (Get-Date).ToString('o')
        raw                    = $null
    }
    if ([string]::IsNullOrWhiteSpace($ExpectedSha256)) {
        $r.state = 'missing'
        $r.detail = '未提供 -ExpectedApprovedTaskConfigSha256（获批任务配置文件自身的 SHA-256，必须取自签署的 G6/G7 执行包）：tasks 指纹不覆盖 connection，文件可被改动而指纹不变 → 拒绝'
        return $r
    }
    $want = ConvertTo-WdDigestHex -Value $ExpectedSha256
    if (-not $want) {
        $r.state = 'invalid'
        $r.expected = $ExpectedSha256
        $r.detail = "ExpectedApprovedTaskConfigSha256 格式非法：$ExpectedSha256（应形如 sha256:<64 hex>）"
        return $r
    }
    $r.expected = $want
    $raw = Read-WdRawFileBytes -Path $Path
    if (-not $raw.ok) {
        $r.detail = "获批任务配置读不到（$($raw.note)）→ 无法核对文件自身摘要（按未通过处理）"
        return $r
    }
    $r.raw = $raw
    $r.path = $raw.path
    $r.size = $raw.size
    $r.actual = $raw.sha256
    $r.verified_before_parse = $true
    if ($raw.sha256 -ne $want) {
        $r.state = 'invalid'
        $r.detail = "获批任务配置文件的 SHA-256（$($raw.sha256)）与签署包批准值（$want）不一致：该文件不是签署时批准的版本 → 拒绝（tasks 指纹相同也不放行：指纹不覆盖 connection）"
        return $r
    }
    $r.state = 'verified'
    $r.ok = $true
    $r.detail = "获批任务配置文件的原始字节 SHA-256 与签署包批准值一致（$want；$($raw.size) 字节；在解析文件之前比对）"
    return $r
}

function Compare-WdConnectionConfig {
    # 比对连接配置（B3）：host / port / TSAP（local_tsap + remote_tsap）/ PDU（pdu_length）。
    # 字段名与 sinumerik-nck 的 connection schema 一致（drivers/sinumerik-nck/src/config.rs:26-41）。
    param($Expected, $Actual)
    $fields = @(
        @{ name = 'host'; kind = 'string' },
        @{ name = 'port'; kind = 'int' },
        @{ name = 'local_tsap'; kind = 'int' },
        @{ name = 'remote_tsap'; kind = 'int' },
        @{ name = 'pdu_length'; kind = 'int' }
    )
    $problems = @()
    $exp = [ordered]@{}
    $act = [ordered]@{}
    foreach ($f in $fields) {
        $n = $f.name
        $ev = $null
        $av = $null
        if ($Expected -and $Expected.PSObject.Properties[$n]) { $ev = $Expected.$n }
        if ($Actual -and $Actual.PSObject.Properties[$n]) { $av = $Actual.$n }
        if ($f.kind -eq 'int') {
            $ei = if ($null -ne $ev -and "$ev" -match '^-?\d+$') { [long]$ev } else { $null }
            $ai = if ($null -ne $av -and "$av" -match '^-?\d+$') { [long]$av } else { $null }
            $exp[$n] = $ei
            $act[$n] = $ai
            if ($null -eq $ai) { $problems += "连接配置 $n 读不到（端点未返回该字段）" }
            elseif ($null -eq $ei) { $problems += "连接配置 $n 在获批配置里缺失或非整数（$ev）" }
            elseif ($ei -ne $ai) { $problems += "连接配置 $n 不一致（期望 $ei，实际 $ai）" }
        }
        else {
            $es = if ($null -ne $ev) { ([string]$ev).Trim() } else { $null }
            $as = if ($null -ne $av) { ([string]$av).Trim() } else { $null }
            $exp[$n] = $es
            $act[$n] = $as
            if ([string]::IsNullOrEmpty($as)) { $problems += "连接配置 $n 读不到（端点未返回该字段）" }
            elseif ([string]::IsNullOrEmpty($es)) { $problems += "连接配置 $n 在获批配置里缺失" }
            elseif ($es -ine $as) { $problems += "连接配置 $n 不一致（期望 $es，实际 $as）" }
        }
    }
    return [ordered]@{ problems = @($problems); expected = $exp; actual = $act }
}

function Get-WdPointKeyMap {
    # 从 tasks 提取"点位 Key → 该点位的规范化完整参数（紧凑 JSON）"映射
    # （B3：核**完整 Key 与参数**，不只数量）。
    # 规则：tasks[].binding.config.points[] 的每个元素是一个点位
    # （mesa_core_types::PointSelection：point_key / kind / parameters，见 crates/core-types/src/point.rs:100-120）；
    # Key 取 point_key，兼容 key / name / id。参数取该元素**整体**（比"只比 parameters"更严格）。
    # 映射键写成 "<task id>/<point key>"：跨任务同名 Key 本身就是配置错误，按任务分组更易定位。
    param($Tasks)
    $map = [ordered]@{}
    foreach ($t in @($Tasks)) {
        if ($null -eq $t) { continue }
        $tid = ''
        if ($t.PSObject.Properties['id']) { $tid = [string]$t.id }
        $points = $null
        if ($t.PSObject.Properties['binding'] -and $null -ne $t.binding -and
            $t.binding.PSObject.Properties['config'] -and $null -ne $t.binding.config -and
            $t.binding.config.PSObject.Properties['points']) {
            $points = @($t.binding.config.points)
        }
        if ($null -eq $points) { continue }
        foreach ($p in $points) {
            if ($null -eq $p) { continue }
            $k = $null
            foreach ($cand in @('point_key', 'key', 'name', 'id')) {
                if ($p.PSObject.Properties[$cand] -and -not [string]::IsNullOrWhiteSpace([string]$p.$cand)) { $k = [string]$p.$cand; break }
            }
            if (-not $k) { continue }
            $canon = ConvertTo-WdCanonicalNode -Node $p
            $map["$tid/$k"] = (ConvertTo-Json -InputObject $canon -Compress -Depth 100)
        }
    }
    return $map
}

function Get-WdTaskPointCount {
    # **当前任务点数** = 实时任务快照里 tasks[].binding.config.points[] 的总数。
    #
    # 为什么不能用 diagnostics.point_count（BLOCKED-01）：
    #   diagnostics.point_count 的来源是**持久化 point_registry 的行数**
    #   （crates/core-api/src/lib.rs:1499 → crates/config-store/src/lib.rs:2459-2473，
    #    `SELECT … FROM point_registry WHERE endpoint_id=?1 AND deleted=0`）；
    #   唯一写点是 assign_point_ids（config-store:2344-2456），由
    #   StorePointIdSource::assign（crates/driver-manager/src/endpoint.rs:206-224，调用点 :213）
    #   在 run_config_flow 的 :943-945（ConfigureTasks :914 之后、ApplyPointMap :968-985 之前）写入。
    #   也就是说：读到的行数只反映"**上次走完 ConfigureTasks 的那次 Start**"留下的残留状态，
    #   与本次要核对的任务配置无关 —— 拿它当"配置正确性"判据，等于用历史残留冒充当前配置。
    #   真正的当前配置只能从 `GET /api/v1/tasks?endpoint=…` 的实时快照里算。
    #
    # 返回 @{ ok; count; tasks; points_arrays; note }：
    #   ok=false 仅当**任务快照根本没有 tasks 数组**（无法核实 ⇒ 调用方按 unverified 处理）；
    #   tasks 存在但没有 binding.config.points[]（含空数组）⇒ ok=true、count=0，
    #   这是"配置确实为 0 个点"，属于**读到但不一致**（invalid），不是读不到。
    param($Tasks)
    $r = [ordered]@{ ok = $false; count = 0; tasks = 0; points_arrays = 0; note = '' }
    if ($null -eq $Tasks) {
        $r.note = '任务快照没有 tasks 数组'
        return $r
    }
    $arr = @($Tasks)
    $r.tasks = $arr.Count
    $r.ok = $true
    foreach ($t in $arr) {
        if ($null -eq $t) { continue }
        if (-not ($t.PSObject.Properties['binding'] -and $null -ne $t.binding -and
                $t.binding.PSObject.Properties['config'] -and $null -ne $t.binding.config -and
                $t.binding.config.PSObject.Properties['points'])) { continue }
        $r.points_arrays++
        $r.count += @($t.binding.config.points | Where-Object { $null -ne $_ }).Count
    }
    if ($r.points_arrays -eq 0) { $r.note = '任务快照里没有任何 tasks[].binding.config.points[]（点数按 0 计）' }
    return $r
}

function Get-WdTaskSnapshot {
    # 只读任务快照：优先 Core 的真实形态 `GET /api/v1/tasks?endpoint=<id>`
    # （crates/core-api/src/lib.rs:2317-2337 → {endpoint_id, revision, tasks}）；
    # 404/405 时回退到 `GET /api/v1/tasks/<id>`（旧桩形态，保持兼容）。
    # 返回 @{ status; revision; tasks; source; uri; http }；调用方据 status 判 unverified。
    param($Ctx, [string]$Endpoint)
    $base = $Ctx.baseUrl
    $out = [ordered]@{ status = 0; revision = $null; tasks = $null; source = ''; uri = ''; http = [ordered]@{} }
    $queryUri = "$base/api/v1/tasks?endpoint=$Endpoint"
    $pathUri = "$base/api/v1/tasks/$Endpoint"
    $script:credNetworkActions++
    $r = Invoke-WebRequest -Uri $queryUri -Method Get -TimeoutSec ([int]$Ctx.observeTimeoutSec) -SkipHttpErrorCheck -ErrorAction Stop
    $out.http['query'] = [int]$r.StatusCode
    if ([int]$r.StatusCode -eq 200) {
        $out.uri = $queryUri; $out.source = 'query'; $out.status = 200
        $o = [string]$r.Content | ConvertFrom-Json
        if ($null -ne $o.revision) { $out.revision = [string]$o.revision }
        if ($null -ne $o.tasks) { $out.tasks = @($o.tasks) }
        return $out
    }
    if ([int]$r.StatusCode -in @(404, 405)) {
        $script:credNetworkActions++
        $r2 = Invoke-WebRequest -Uri $pathUri -Method Get -TimeoutSec ([int]$Ctx.observeTimeoutSec) -SkipHttpErrorCheck -ErrorAction Stop
        $out.http['path'] = [int]$r2.StatusCode
        $out.uri = $pathUri; $out.source = 'path'; $out.status = [int]$r2.StatusCode
        if ([int]$r2.StatusCode -eq 200) {
            $o2 = [string]$r2.Content | ConvertFrom-Json
            if ($null -ne $o2.revision) { $out.revision = [string]$o2.revision }
            if ($null -ne $o2.tasks) { $out.tasks = @($o2.tasks) }
        }
        return $out
    }
    $out.uri = $queryUri; $out.source = 'query'; $out.status = [int]$r.StatusCode
    return $out
}

function Test-WdCredTaskConfig {
    # 任务配置核验（B3 之后：核对**完整配置**；BLOCKED-01 之后：点数取自**实时任务快照**）：
    #   ① revision 与**当前任务点数**（点数 = 实时快照 tasks[].binding.config.points[] 总数）
    #   ② 连接配置 host / port / TSAP(local_tsap + remote_tsap) / PDU(pdu_length)
    #   ③ 获批点位的**完整 Key 与参数**（tasks[].binding.config.points[]，逐点比对，不只数量）
    #   ④ 任务配置**指纹**（规范化 tasks 数组的 SHA-256）与批准版本一致
    #   ⓪（G2，**最先做**）获批配置文件**自身**的 SHA-256 必须等于签署包批准值，
    #      且在**解析文件之前**对**原始字节**计算 —— 指纹不覆盖 connection，只有文件摘要能挡住
    #      "connection 被改而指纹不变"的篡改。
    # 任一项读不到 ⇒ unverified；读到但不一致 ⇒ invalid。两者都不得进入 FIRED（不发 Start）。
    #
    # BLOCKED-01（最小修复，只改配置门语义）：`diagnostics.point_count` **不再**参与判定。
    #   它的来源是持久化 point_registry 行数（core-api:1499 → config-store:2459-2473；唯一写点
    #   assign_point_ids config-store:2344-2456，由 run_config_flow:943-945 在 ConfigureTasks
    #   之后写入），因此 FIRED 前读到的是"上次走完 ConfigureTasks 的那次 Start"的**残留行数**，
    #   与当前任务配置无关。它现在只作为**独立的运行状态观测值**记入证据
    #   （actual.diagnostics_point_count / summary.task_config_observation），不参与配置门判定。
    param($Ctx, [string]$ExpectedEndpoint, [string]$ExpectedRevision, [int]$ExpectedPoints,
        [string]$ApprovedPath, [string]$ExpectedFingerprint, [string]$ExpectedApprovedSha256 = '')
    $expected = [ordered]@{
        endpoint_id                = $ExpectedEndpoint
        revision                   = $ExpectedRevision
        # 语义：**获批任务配置的点数**（与实时任务快照比对），不是 diagnostics.point_count。
        task_point_count           = $ExpectedPoints
        approved_task_config       = $ApprovedPath
        tasks_fingerprint          = $ExpectedFingerprint
        approved_config_sha256     = $ExpectedApprovedSha256
        diagnostics_point_count_role = 'observed-only：运行状态残留值（持久化 point_registry 行数），非当前任务点数，不参与配置门判定'
    }
    # ── 输入完整性：缺任一项都不得放行（B3 的期望值不是"可选项"）──────────────────
    if ([string]::IsNullOrWhiteSpace($ExpectedRevision)) {
        return (New-WdCredItem 'task_config' 'missing' '未提供 -ExpectedTaskRevision（期望的任务 revision）' $expected $null)
    }
    if ($ExpectedPoints -lt 0) {
        return (New-WdCredItem 'task_config' 'missing' '未提供 -ExpectedPointCount（获批任务配置的点数，与实时任务快照比对）' $expected $null)
    }
    if ([string]::IsNullOrWhiteSpace($ApprovedPath)) {
        return (New-WdCredItem 'task_config' 'missing' '未提供 -ApprovedTaskConfig（获批任务配置：连接配置 + 完整点位 Key/参数）——只比 revision/点数不足以证明与获批配置一致' $expected $null)
    }
    if ([string]::IsNullOrWhiteSpace($ExpectedFingerprint)) {
        return (New-WdCredItem 'task_config' 'missing' '未提供 -ExpectedTaskFingerprint（批准版本的任务配置指纹 sha256:<64 hex>）' $expected $null)
    }
    if ($ExpectedFingerprint -notmatch '^(?i)sha256:[0-9a-f]{64}$') {
        return (New-WdCredItem 'task_config' 'invalid' "ExpectedTaskFingerprint 格式非法：$ExpectedFingerprint（应形如 sha256:<64 hex>）" $expected $null)
    }
    $wantFp = ($ExpectedFingerprint -replace '^(?i)sha256:', '').ToLowerInvariant()

    # ── ⓪ G2：获批配置文件**自身**必须先与签署包批准值比对（先算摘要、后解析）──────
    $approvedIntegrity = Test-WdApprovedFileIntegrity -Path $ApprovedPath -ExpectedSha256 $ExpectedApprovedSha256
    if ($approvedIntegrity.state -ne 'verified') {
        return (New-WdCredItem 'task_config' $approvedIntegrity.state $approvedIntegrity.detail $expected ([ordered]@{ approved_file = $approvedIntegrity }) -Label 'REFUSED_TASKCONFIG_INTEGRITY')
    }

    # ── ① 本地：获批配置必须自证是"批准版本"（不涉及网络）──────────────────────
    # 解析的是**刚校验过摘要的那份字节**（不是重新按路径读一遍），排除"比对与解析不同源"。
    $approved = Read-WdApprovedTaskConfig -Path $ApprovedPath -Bytes $approvedIntegrity.raw.bytes -FileSha256 $approvedIntegrity.actual
    if (-not $approved.ok) {
        return (New-WdCredItem 'task_config' 'unverified' ("获批任务配置不可用（{0}）→ 无法核验任务配置（按未通过处理）" -f $approved.note) $expected ([ordered]@{ approved = $approved; approved_file = $approvedIntegrity }))
    }
    $approvedFp = [string]$approved.tasks_fingerprint
    if ($approvedFp -ne "sha256:$wantFp") {
        $actual0 = [ordered]@{ approved = $approved; approved_file = $approvedIntegrity; approved_fingerprint = $approvedFp; expected_fingerprint = "sha256:$wantFp"; http = [ordered]@{} }
        return (New-WdCredItem 'task_config' 'invalid' ("获批任务配置文件的指纹（{0}）与 -ExpectedTaskFingerprint（sha256:{1}）不一致：该文件不是获批版本（或已被改动）→ 拒绝" -f $approvedFp, $wantFp) $expected $actual0)
    }

    # ── ② 网络：三条**配置面**只读 GET ───────────────────────────────────────────
    # tasks（revision + 完整点位/参数 + **当前任务点数**）、endpoints/{id}（connection：host/port/
    # TSAP/PDU —— B3 必需；Core 只在这条路径暴露连接配置）、diagnostics（endpoint_id 观测；
    # 其 point_count 仅作运行状态观测记录，不参与判定）。
    $base = $Ctx.baseUrl
    $diagUri = "$base/api/v1/endpoints/$ExpectedEndpoint/diagnostics"
    $epUri = "$base/api/v1/endpoints/$ExpectedEndpoint"
    $actual = [ordered]@{
        endpoint_id             = $null
        revision                = $null
        # 当前任务点数：**配置门判据**，来自实时任务快照 tasks[].binding.config.points[]。
        task_point_count        = $null
        task_count              = $null
        task_points_arrays      = $null
        # diagnostics.point_count：**只作运行状态观测**（持久化 point_registry 残留行数），
        # 不参与配置门判定 —— 见函数头 BLOCKED-01 说明。
        diagnostics_point_count = $null
        connection              = $null
        tasks_fingerprint       = $null
        approved_fingerprint    = $approvedFp
        approved_file           = $approvedIntegrity
        point_keys              = [ordered]@{}
        tasks_source            = ''
        tasks_payload           = $null
        http                    = [ordered]@{}
    }
    $script:credNetworkActions = 0
    try {
        $script:credNetworkActions++
        $r1 = Invoke-WebRequest -Uri $diagUri -Method Get -TimeoutSec ([int]$Ctx.observeTimeoutSec) -SkipHttpErrorCheck -ErrorAction Stop
        $actual.http['diagnostics'] = [int]$r1.StatusCode
        if ([int]$r1.StatusCode -ne 200) {
            return (New-WdCredItem 'task_config' 'unverified' "diagnostics 读不到（GET $diagUri → HTTP $($r1.StatusCode)），无法核对端点身份（按未通过处理）" $expected $actual)
        }
        $o1 = [string]$r1.Content | ConvertFrom-Json
        if ($o1.PSObject.Properties['endpoint_id']) { $actual.endpoint_id = [string]$o1.endpoint_id }
        # 只记录，不判定（BLOCKED-01）：这是"上次走完 ConfigureTasks 的那次 Start"的残留行数。
        if ($null -ne $o1.point_count) { $actual.diagnostics_point_count = [int]$o1.point_count }

        $snap = Get-WdTaskSnapshot -Ctx $Ctx -Endpoint $ExpectedEndpoint
        $actual.http['tasks'] = [int]$snap.status
        $actual.tasks_source = [string]$snap.source
        if ([int]$snap.status -ne 200) {
            return (New-WdCredItem 'task_config' 'unverified' ("任务快照读不到（GET {0} → HTTP {1}），无法核验 revision / 当前任务点数 / 点位 / 指纹（按未通过处理）" -f $snap.uri, $snap.status) $expected $actual)
        }
        if ($null -ne $snap.revision) { $actual.revision = [string]$snap.revision }
        if ($null -ne $snap.tasks) {
            $fp = Get-WdTasksFingerprint -Tasks $snap.tasks
            $actual.tasks_fingerprint = $fp.fingerprint
            $actual.tasks_payload = $fp.payload
            $actual.point_keys = Get-WdPointKeyMap -Tasks $snap.tasks
        }
        # 当前任务点数（判据）：必须来自**实时快照**，读不到就保持 $null ⇒ 后面判 unverified。
        $livePoints = Get-WdTaskPointCount -Tasks $snap.tasks
        if ($livePoints.ok) {
            $actual.task_point_count = [int]$livePoints.count
            $actual.task_count = [int]$livePoints.tasks
            $actual.task_points_arrays = [int]$livePoints.points_arrays
        }

        $script:credNetworkActions++
        $r3 = Invoke-WebRequest -Uri $epUri -Method Get -TimeoutSec ([int]$Ctx.observeTimeoutSec) -SkipHttpErrorCheck -ErrorAction Stop
        $actual.http['endpoint'] = [int]$r3.StatusCode
        if ([int]$r3.StatusCode -eq 200) {
            $o3 = [string]$r3.Content | ConvertFrom-Json
            if ($o3.PSObject.Properties['connection'] -and $null -ne $o3.connection) { $actual.connection = $o3.connection }
        }
    }
    catch {
        return (New-WdCredItem 'task_config' 'unverified' "任务配置核验的只读 GET 失败：$($_.Exception.Message)（按未通过处理）" $expected $actual)
    }

    $problems = @()
    # ② 基础项（原有）
    if (-not $actual.endpoint_id) { $problems += 'diagnostics 未返回 endpoint id' }
    elseif ($actual.endpoint_id -ne $ExpectedEndpoint) { $problems += "endpoint id 不一致（期望 $ExpectedEndpoint，实际 $($actual.endpoint_id)）" }
    if ($null -eq $actual.revision) { $problems += '任务 revision 读不到' }
    elseif ($actual.revision -ne [string]$ExpectedRevision) { $problems += "任务 revision 不一致（期望 $ExpectedRevision，实际 $($actual.revision)）" }
    # ② 当前任务点数（**配置门判据**）：实时任务快照 tasks[].binding.config.points[] 的总数。
    #    与 -ExpectedPointCount（获批任务配置的点数）比对。
    #    为什么不是 diagnostics.point_count：见函数头 BLOCKED-01 —— 那是 point_registry 的残留行数，
    #    与当前任务配置无关；拿它判定会把"历史残留"误判成"当前配置"。
    if ($null -eq $actual.task_point_count) { $problems += '当前任务点数读不到（任务快照里没有 tasks[].binding.config.points[]）' }
    elseif ([int]$actual.task_point_count -ne $ExpectedPoints) {
        $problems += "当前任务点数不一致（期望 $ExpectedPoints，实际 $($actual.task_point_count)；取自实时任务快照 tasks[].binding.config.points[]，非 diagnostics.point_count）"
    }
    # 获批文件自身也必须自洽（防止拿错端点/版本的批准文件）
    if ($approved.endpoint_id -and $approved.endpoint_id -ne $ExpectedEndpoint) {
        $problems += "获批配置的 endpoint_id 不一致（期望 $ExpectedEndpoint，获批文件 $($approved.endpoint_id)）"
    }
    if ($approved.revision -and $approved.revision -ne [string]$ExpectedRevision) {
        $problems += "获批配置的 revision 不一致（期望 $ExpectedRevision，获批文件 $($approved.revision)）"
    }
    # ③ 连接配置（host / port / TSAP / PDU）
    if ($null -eq $actual.connection) {
        $problems += "端点未返回 connection（GET $epUri → HTTP $($actual.http['endpoint'])）：无法核对 host/port/TSAP/PDU"
    }
    $connCmp = Compare-WdConnectionConfig -Expected $approved.connection -Actual $actual.connection
    foreach ($p in $connCmp.problems) { $problems += $p }
    # ④ 点位完整 Key 与参数（逐点比对，不只数量）
    $expPoints = Get-WdPointKeyMap -Tasks $approved.tasks
    $actPoints = $actual.point_keys
    # 获批配置里的**点位总数**必须等于 -ExpectedPointCount（tasks 条数 ≠ 点位数：一个 task 可含多点）。
    if ($expPoints.Count -ne $ExpectedPoints) {
        $problems += "获批配置的点位总数与 -ExpectedPointCount 不一致（获批 $($expPoints.Count) 个 / 期望 $ExpectedPoints 个）"
    }
    foreach ($k in $expPoints.Keys) {
        if (-not $actPoints.Contains($k)) { $problems += "获批点位缺失：$k（实际配置里没有这个 Key）" }
        elseif ($expPoints[$k] -ne $actPoints[$k]) { $problems += "点位参数不一致：$k（获批 $($expPoints[$k])；实际 $($actPoints[$k])）" }
    }
    foreach ($k in $actPoints.Keys) {
        if (-not $expPoints.Contains($k)) { $problems += "实际配置多出点位：$k（获批配置里没有）" }
    }
    if ($expPoints.Count -eq 0 -and $approved.tasks.Count -gt 0) {
        $problems += '获批配置的 tasks 里没有可识别的点位（binding.config.points[]）→ 无法证明点位 Key/参数一致'
    }
    # ⑤ 任务配置指纹
    if ($null -eq $actual.tasks_fingerprint) { $problems += '任务配置指纹算不出来（任务快照无 tasks 数组）' }
    elseif ($actual.tasks_fingerprint -ne "sha256:$wantFp") {
        $problems += "任务配置指纹不一致（期望 sha256:$wantFp，实际 $($actual.tasks_fingerprint)）"
    }
    if ($problems.Count -gt 0) {
        # 读不到（缺值）算 unverified；读到但不一致算 invalid —— 两者都不放行。
        $unreadable = @($problems | Where-Object { $_ -match '读不到|未返回|算不出' })
        $state = if ($unreadable.Count -gt 0) { 'unverified' } else { 'invalid' }
        return (New-WdCredItem 'task_config' $state ("任务配置核验不通过（完整配置核对）：" + ($problems -join '；')) $expected $actual)
    }
    $okDetail = ("完整任务配置一致：endpoint={0}；revision={1}；当前任务点数={2}（取自实时任务快照 tasks[].binding.config.points[]，共 {3} 个 task）；点位 {4} 个（Key 与参数逐点一致）；连接 {5}:{6} TSAP={7}/{8} PDU={9}；指纹 {10}；diagnostics_point_count={11}（**运行状态观测值**：持久化 point_registry 残留行数，非当前任务点数，不参与本判定）" -f `
            $ExpectedEndpoint, $actual.revision, $actual.task_point_count, $actual.task_count, $expPoints.Count,
        $connCmp.actual.host, $connCmp.actual.port, $connCmp.actual.local_tsap, $connCmp.actual.remote_tsap,
        $connCmp.actual.pdu_length, $actual.tasks_fingerprint, $(if ($null -eq $actual.diagnostics_point_count) { '（读不到）' } else { [string]$actual.diagnostics_point_count }))
    return (New-WdCredItem 'task_config' 'verified' $okDetail $expected $actual)
}

function Test-WdCredentials {
    # 汇总四项凭证。CheckNetwork=false 时只做静态校验（不建立任何连接）。
    # IncludeAuthorization=false 用于 ARMED：arm 不要求"显式授权"（那是 FIRED 的门），
    # 只校验 G6/G7/任务配置——否则 ARMED 永远无法给出"可进入 FIRED"的结论。
    param([bool]$CheckNetwork, [bool]$IncludeAuthorization = $true)
    $items = [ordered]@{}
    if (-not $IncludeAuthorization) {
        $items['authorized'] = New-WdCredItem 'authorized' 'not_checked' ("arm 不要求显式授权（进入 fire 时必须 -Authorized + -AuthorizedBy；当前 Authorized={0} AuthorizedBy={1}）" -f $Authorized, $(if ($AuthorizedBy) { $AuthorizedBy } else { '（空）' })) ([ordered]@{ authorized = [bool]$Authorized; authorized_by = $AuthorizedBy }) $null
    }
    elseif ($Authorized -and -not [string]::IsNullOrWhiteSpace($AuthorizedBy)) {
        $items['authorized'] = New-WdCredItem 'authorized' 'verified' "显式授权：授权人=$AuthorizedBy" ([ordered]@{ authorized = $true; authorized_by = $AuthorizedBy }) $null
    }
    elseif (-not $Authorized) {
        $items['authorized'] = New-WdCredItem 'authorized' 'missing' '未给出 -Authorized（fire 必须显式授权，不允许默认/继承）' ([ordered]@{ authorized = $false; authorized_by = $AuthorizedBy }) $null
    }
    else {
        $items['authorized'] = New-WdCredItem 'authorized' 'missing' '缺少授权人标识 -AuthorizedBy（授权必须可追溯）' ([ordered]@{ authorized = $true; authorized_by = $AuthorizedBy }) $null
    }
    if (-not $IncludeAuthorization) {
        $items['authz_ticket'] = New-WdCredItem 'authz_ticket' 'not_checked' ("arm 不要求授权凭证编号（进入 fire 时必须提供 -AuthzTicket；当前={0}）" -f $(if ($AuthzTicket) { $AuthzTicket } else { '（空）' })) $AuthzTicket $null
    }
    elseif ([string]::IsNullOrWhiteSpace($AuthzTicket)) {
        $items['authz_ticket'] = New-WdCredItem 'authz_ticket' 'missing' '未提供 -AuthzTicket（授权凭证编号）' $null $null
    }
    else {
        $items['authz_ticket'] = New-WdCredItem 'authz_ticket' 'verified' "授权凭证编号=$AuthzTicket" $AuthzTicket $null
    }
    $items['g6_signoff'] = Test-WdCredG6 -Signoff $G6Signoff -SignedAt $G6SignedAt
    $items['g7_deploy_version'] = Test-WdCredG7 -Provided $G7DeployVersion -BinaryPath $G7BinaryPath -ExpectedManifestSha256 $ExpectedManifestSha256
    if ($CheckNetwork) {
        $items['task_config'] = Test-WdCredTaskConfig -Ctx $ctx -ExpectedEndpoint $EndpointId -ExpectedRevision $ExpectedTaskRevision -ExpectedPoints $ExpectedPointCount -ApprovedPath $ApprovedTaskConfig -ExpectedFingerprint $ExpectedTaskFingerprint -ExpectedApprovedSha256 $ExpectedApprovedTaskConfigSha256
    }
    else {
        # 静态阶段也核对"获批配置文件自身"的摘要（G2）：这是**纯本地文件检查、不需要网络**，
        # 因此必须在这一阶段就成立 —— 好处是 G2 的拒绝发生在任何连接建立之前（network_actions = 0），
        # 而不是"先连上端点再去发现批准文件是假的"。
        $fileIntegrity = Test-WdApprovedFileIntegrity -Path $ApprovedTaskConfig -ExpectedSha256 $ExpectedApprovedTaskConfigSha256
        $items['approved_task_config_integrity'] = New-WdCredItem 'approved_task_config_integrity' $fileIntegrity.state $fileIntegrity.detail `
        ([ordered]@{ path = $ApprovedTaskConfig; expected = $ExpectedApprovedTaskConfigSha256 }) ([ordered]@{ file = $fileIntegrity }) `
        $(if ($fileIntegrity.state -eq 'verified') { '' } else { 'REFUSED_TASKCONFIG_INTEGRITY' })
        $items['task_config'] = New-WdCredItem 'task_config' 'not_checked' '静态校验阶段未做任务配置核验（需先通过授权边界后才发起只读 GET；获批配置**文件自身**的摘要已在 approved_task_config_integrity 里核对）' ([ordered]@{ endpoint_id = $EndpointId; revision = $ExpectedTaskRevision; task_point_count = $ExpectedPointCount; approved_task_config = $ApprovedTaskConfig; tasks_fingerprint = $ExpectedTaskFingerprint; approved_config_sha256 = $ExpectedApprovedTaskConfigSha256 }) $null
    }
    # not_checked 是"静态阶段还没做"的占位（任务配置核验需要只读 GET），不算失败项。
    $failing = @($items.Keys | Where-Object { $items[$_].state -notin @('verified', 'not_checked') })
    return [ordered]@{
        overall = $(if ($failing.Count -eq 0) { 'verified' } else { 'rejected' })
        failing = $failing
        items   = $items
    }
}


#endregion

#region ── ARMED：只自检 + 报告，零副作用（P0-1 / F1 / F2）──────────────────────
# 进入本分支时：①未建任何执行流；②未写 t0.stamp；③除下面的只读 GET（可选 -SelfTest 的幂等
# POST /stop）外没有任何网络动作。ARMED 结束后脚本**直接退出**，不存在"继续往下走就 FIRED"的路径。
if ($Mode -eq 'arm') {
    Add-WdLog $ctx 'INFO' 'ARMED：开始自检（不启动执行流、不生成 T0、不发 Start）'
    Write-Host '[ARMED] 开始自检：不启动执行流、不生成 T0、不发 Start'

    # ① 只读探活：确认"明确指定的离线模拟端点"是否可达，并取一次快照（不写任何状态）
    $uri = "$($ctx.baseUrl)/api/v1/endpoints/$EndpointId"
    $probe = [ordered]@{
        reachable       = $false
        http_status     = 0
        state           = $null
        desired_running = $null
        epoch           = $null
        driver_id       = $null
        device_host     = $null
        device_port     = $null
        err             = $null
    }
    $networkActions = [System.Collections.Generic.List[string]]::new()
    try {
        $networkActions.Add("GET $uri（只读快照）")
        $resp = Invoke-WebRequest -Uri $uri -Method Get -TimeoutSec $ObserveTimeoutSec -SkipHttpErrorCheck -ErrorAction Stop
        $probe.http_status = [int]$resp.StatusCode
        if ($probe.http_status -eq 200) {
            $o = [string]$resp.Content | ConvertFrom-Json
            $probe.reachable = $true
            if ($o.runtime.PSObject.Properties['state']) { $probe.state = [string]$o.runtime.state }
            if ($o.runtime.PSObject.Properties['epoch']) { $probe.epoch = [string]$o.runtime.epoch }
            if ($o.PSObject.Properties['desired_running']) { $probe.desired_running = $o.desired_running }
            if ($o.PSObject.Properties['driver_id']) { $probe.driver_id = [string]$o.driver_id }
            if ($o.PSObject.Properties['connection']) {
                if ($o.connection.PSObject.Properties['host']) { $probe.device_host = [string]$o.connection.host }
                if ($o.connection.PSObject.Properties['port']) { $probe.device_port = [int]$o.connection.port }
            }
        }
        else {
            $probe.err = "HTTP $($probe.http_status)"
        }
    }
    catch {
        $probe.err = $_.Exception.Message
    }
    Add-WdLog $ctx $(if ($probe.reachable) { 'INFO' } else { 'ERROR' }) ("ARMED 探活：reachable={0} http={1} state={2} desired={3} err={4}" -f $probe.reachable, $probe.http_status, $probe.state, $probe.desired_running, $probe.err)

    # ② 四项凭证**校验**（ARMED 也做，只读）：G6 格式 / G7 版本比对 / 任务配置只读核对 / 显式授权。
    # 端点可达时才做任务配置核验（需要只读 GET）；不可达则记 unverified。
    $creds = Test-WdCredentials -CheckNetwork:$probe.reachable -IncludeAuthorization:$false
    Add-WdLog $ctx $(if ($creds.overall -eq 'verified') { 'INFO' } else { 'WARN' }) ("ARMED 凭证校验：overall={0}；未通过={1}" -f $creds.overall, ($(if ($creds.failing.Count -gt 0) { $creds.failing -join ',' } else { '（无）' })))
    foreach ($k in $creds.items.Keys) {
        $it = $creds.items[$k]
        Add-WdLog $ctx $(if ($it.state -eq 'verified') { 'INFO' } else { 'WARN' }) ("凭证 {0}：{1}；{2}" -f $k, $it.state, $it.detail)
    }

    # ② 可选通路自检（G6-ARMED-4）：幂等 POST …/stop，要求 200 且 was_running=false
    $selfTestRec = $null
    if ($SelfTest) {
        $selfTestRec = [ordered]@{
            t_local     = (Get-Date).ToString('o')
            pid         = $PID
            base_url    = $ctx.baseUrl
            endpoint_id = $EndpointId
            pre_state   = $probe.state
            http_status = 0
            body        = $null
            ok          = $false
            reason      = $null
        }
        if (-not $probe.reachable) {
            $selfTestRec.reason = '端点不可达，自检无法执行'
        }
        elseif ($probe.state -ne 'STOPPED') {
            # 纪律：自检只能在 Start 之前做（此时端点本已 STOPPED）；运行中自检会制造"已验证"错觉。
            $selfTestRec.reason = "端点当前 state=$($probe.state)，不是 STOPPED；自检拒绝执行（§4.6.1 纪律）"
        }
        else {
            try {
                $stopUri = "$uri/stop"
                $networkActions.Add("POST $stopUri（幂等自检写）")
                $r = Invoke-WebRequest -Uri $stopUri -Method Post -TimeoutSec $StopTimeoutSec -SkipHttpErrorCheck -ErrorAction Stop
                $selfTestRec.http_status = [int]$r.StatusCode
                $selfTestRec.body = [string]$r.Content
                $bodyObj = $null
                try { $bodyObj = $selfTestRec.body | ConvertFrom-Json } catch { }
                $wasRunning = $null
                if ($bodyObj -and $bodyObj.PSObject.Properties['was_running']) { $wasRunning = $bodyObj.was_running }
                if ($selfTestRec.http_status -eq 200 -and $wasRunning -eq $false) {
                    $selfTestRec.ok = $true
                    $selfTestRec.reason = 'POST .../stop 返回 200 且 was_running=false（幂等，core-api:2169-2175）'
                }
                else {
                    $selfTestRec.reason = "自检失败：http=$($selfTestRec.http_status) was_running=$wasRunning（期望 200/false）"
                }
            }
            catch {
                $selfTestRec.reason = "自检异常：$($_.Exception.Message)"
            }
        }
        [System.IO.File]::WriteAllText($selfTestPath, ($selfTestRec | ConvertTo-Json -Depth 6), [System.Text.UTF8Encoding]::new($false))
        Add-WdLog $ctx $(if ($selfTestRec.ok) { 'INFO' } else { 'ERROR' }) "自检：$($selfTestRec.reason)"
        Write-Host ("[SelfTest] {0}  {1}" -f $(if ($selfTestRec.ok) { 'OK' } else { 'FAIL' }), $selfTestRec.reason)
    }

    # ③ 保持 ArmHoldSec 秒：证明"等待超过旧版 1.5 s 也不会 FIRED"
    $holdSw = [System.Diagnostics.Stopwatch]::StartNew()
    if ($ArmHoldSec -gt 0) { Start-Sleep -Seconds $ArmHoldSec }
    $holdMs = [math]::Round($holdSw.Elapsed.TotalMilliseconds, 1)
    $startDispatched = $false   # 结构保证：arm 分支里没有 Start 代码路径
    $flowsStarted = 0
    Add-WdLog $ctx 'INFO' ("ARMED 保持 {0:N0} ms 完毕（≥1500 ms = {1}）：start_dispatched={2}，flows_started={3}，t0_written={4}" -f $holdMs, ($holdMs -ge 1500), $startDispatched, $flowsStarted, (Test-Path $t0Path))

    $armExit = 3
    $armLabel = 'ARMED_ENDPOINT_UNREACHABLE'
    if (-not $probe.reachable) { $armExit = 3; $armLabel = 'ARMED_ENDPOINT_UNREACHABLE' }
    elseif ($SelfTest -and -not $selfTestRec.ok) { $armExit = 2; $armLabel = 'ARMED_SELFTEST_FAIL' }
    elseif ($creds.overall -ne 'verified') { $armExit = 1; $armLabel = 'ARMED_INPUTS_MISSING' }
    else { $armExit = 0; $armLabel = 'ARMED_READY' }

    # BLOCKED-01：配置门的**判据**（实时任务快照算出的当前任务点数）与**运行状态观测值**
    # （diagnostics.point_count）必须在证据里分开呈现，避免把残留观测值当成配置结论。
    $armTaskItem = $creds.items['task_config']
    $armTaskActual = Get-WdProp $armTaskItem 'actual'

    $armReport = [ordered]@{
        mode                 = 'arm'
        pid                  = $PID
        ps_version           = [string]$PSVersionTable.PSVersion
        host                 = $hostName
        started_local        = $procStart.ToString('o')
        base_url             = $ctx.baseUrl
        endpoint_id          = $EndpointId
        offline_boundary     = $boundary
        credentials          = $creds
        # BLOCKED-01：配置门判据 = 实时任务快照点数；diagnostics.point_count 仅为运行状态观测。
        task_config_observation = [ordered]@{
            expected_task_point_count        = $ExpectedPointCount
            task_point_count_live            = (Get-WdProp $armTaskActual 'task_point_count')
            task_count_live                  = (Get-WdProp $armTaskActual 'task_count')
            diagnostics_point_count_observed = (Get-WdProp $armTaskActual 'diagnostics_point_count')
            diagnostics_point_count_role     = 'observed-only：运行状态残留值（持久化 point_registry 行数，core-api:1499 ← config-store:2459-2473），非当前任务点数，不参与配置门判定'
            task_config_state                = (Get-WdProp $armTaskItem 'state')
        }
        endpoint_probe       = $probe
        selftest             = $selfTestRec
        arm                  = [ordered]@{
            hold_ms            = $holdMs
            hold_ge_1_5s       = ($holdMs -ge 1500)
            start_dispatched   = $startDispatched
            flows_started      = $flowsStarted
            t0_written         = (Test-Path $t0Path)
            network_actions    = @($networkActions)
            side_effects       = @($networkActions | Where-Object { $_ -like 'POST*' })
        }
        ready_for_fire       = ($armExit -eq 0)
        next_step            = '若 ready_for_fire=true，运行 fire 需另行显式：-Mode fire -Authorized -AuthorizedBy … -AuthzTicket … -G6Signoff … -G6SignedAt … -G7DeployVersion … -ExpectedManifestSha256 …（manifest 形式时）-ExpectedTaskRevision … -ExpectedPointCount … -ApprovedTaskConfig … -ExpectedApprovedTaskConfigSha256 … -ExpectedTaskFingerprint …'
        verdict              = $armLabel
        exit_code            = $armExit
        out_dir              = $OutDir
    }
    [System.IO.File]::WriteAllText($armReportPath, ($armReport | ConvertTo-Json -Depth 10), [System.Text.UTF8Encoding]::new($false))

    Write-Host ''
    Write-Host '================ stop-watchdog ARMED 结果 ================'
    Write-Host ("PID             : {0}（独立进程）" -f $PID)
    Write-Host ("BaseUrl         : {0}（授权边界：{1}）" -f $ctx.baseUrl, $boundary.reason)
    Write-Host ("Endpoint        : {0}" -f $EndpointId)
    Write-Host ("探活            : reachable={0} http={1} state={2} desired_running={3} epoch={4}" -f $probe.reachable, $probe.http_status, $probe.state, $probe.desired_running, $probe.epoch)
    Write-Host '凭证校验（四项，缺一不得进入 FIRED）:'
    foreach ($k in $creds.items.Keys) {
        $it = $creds.items[$k]
        $flag = switch ($it.state) { 'verified' { 'OK     ' } 'missing' { 'MISSING' } 'invalid' { 'INVALID' } 'unverified' { 'UNKNWN ' } default { 'N/A    ' } }
        Write-Host ("  {0,-18} [{1}] {2}" -f $k, $flag, $it.detail)
    }
    if ($selfTestRec) { Write-Host ("自检            : {0}  {1}" -f $(if ($selfTestRec.ok) { 'OK' } else { 'FAIL' }), $selfTestRec.reason) }
    Write-Host ("保持            : {0:N0} ms（≥1500 ms = {1}）" -f $holdMs, ($holdMs -ge 1500))
    Write-Host '副作用          : 未启动执行流 / 未写 t0.stamp / 未发 Start'
    Write-Host ("网络动作        : {0}" -f $(if ($networkActions.Count -eq 0) { '无' } else { ($networkActions -join '；') }))
    Write-Host ("判定            : {0}" -f $armLabel)
    Write-Host ("是否可以进入 FIRED : {0}" -f $(if ($armExit -eq 0) { '是（需另行显式授权执行 fire）' } else { '否' }))
    Write-Host ("退出码          : {0}" -f $armExit)
    Write-Host ("证据目录        : {0}" -f $OutDir)
    Write-Host '========================================================='

    $ctx.shutdown.Set()
    exit $armExit
}
#endregion

#region ── FIRED 前置条件：显式授权 + 四项凭证**校验通过**（F2）───────────────────
# 到这里说明 Mode=fire。分三步，**任一步不通过都不进入 FIRED（不发 Start）**：
#   ① 静态凭证（不建连接）：授权开关/授权人/凭证号/G6 格式/G7 与二进制或受信清单指纹比对
#      （G1：清单**自身**也要与签署包批准值比对）/获批配置文件**自身**摘要（G2）；
#   ② 授权边界：静态凭证通过 ⇒ 放行任意 host（含生产 mesad 8132 与现场地址）；
#   ③ 任务配置核验（只读 GET）：endpoint 存在性 / 任务 revision / 当前任务点数（实时任务快照） / 连接 / 点位 / 指纹。
$creds = Test-WdCredentials -CheckNetwork:$false
Add-WdLog $ctx 'INFO' ("fire 静态凭证校验：overall={0}；未通过={1}" -f $creds.overall, $(if ($creds.failing.Count -gt 0) { $creds.failing -join ',' } else { '（无）' }))
if ($creds.overall -ne 'verified') {
    $missing = @($creds.failing | Where-Object { $creds.items[$_].state -eq 'missing' })
    # 拒绝标签优先级：
    #   ① 显式授权缺失（-Authorized / -AuthzTicket）⇒ REFUSED_NOT_AUTHORIZED（这是最外层的门，先报它）；
    #   ② 凭证项自己给出的**精确标签**（G1 ⇒ REFUSED_MANIFEST_INTEGRITY / G2 ⇒ REFUSED_TASKCONFIG_INTEGRITY）；
    #   ③ 其余缺失 ⇒ REFUSED_NOT_AUTHORIZED；④ 其余 ⇒ REFUSED_CREDENTIAL_INVALID。
    # 这样"清单/批准文件自身未被独立验证"不会被笼统归成"缺授权"，而"根本就没授权"也不会被
    # 后面的完整性标签掩盖。
    $authMissing = @($creds.failing | Where-Object { $_ -in @('authorized', 'authz_ticket') -and $creds.items[$_].state -eq 'missing' })
    $labelled = @($creds.failing | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$creds.items[$_].label) })
    $label = if ($authMissing.Count -gt 0) { 'REFUSED_NOT_AUTHORIZED' }
    elseif ($labelled.Count -gt 0) { [string]$creds.items[$labelled[0]].label }
    elseif ($missing.Count -gt 0) { 'REFUSED_NOT_AUTHORIZED' }
    else { 'REFUSED_CREDENTIAL_INVALID' }
    $detailLines = @($creds.failing | ForEach-Object { "$_=$($creds.items[$_].state)：$($creds.items[$_].detail)" })
    Write-WdRefusal -Code 40 -Label $label -Reason ("四项凭证未全部通过校验 → 不进入 FIRED：" + ($detailLines -join '；')) -Extra @{ credentials = $creds }
}
$boundary.allowed = $true
$boundary.decided_by = 'fire：静态凭证校验通过'
$boundary.reason = "fire 凭证已校验通过 → 放行 $($urlInfo.host):$($urlInfo.port)"
Add-WdLog $ctx 'INFO' "授权边界放行：$($boundary.reason)"

$taskCred = Test-WdCredTaskConfig -Ctx $ctx -ExpectedEndpoint $EndpointId -ExpectedRevision $ExpectedTaskRevision -ExpectedPoints $ExpectedPointCount -ApprovedPath $ApprovedTaskConfig -ExpectedFingerprint $ExpectedTaskFingerprint -ExpectedApprovedSha256 $ExpectedApprovedTaskConfigSha256
$creds.items['task_config'] = $taskCred
$creds.failing = @($creds.items.Keys | Where-Object { $creds.items[$_].state -ne 'verified' })
$creds.overall = if ($creds.failing.Count -eq 0) { 'verified' } else { 'rejected' }
Add-WdLog $ctx $(if ($taskCred.state -eq 'verified') { 'INFO' } else { 'ERROR' }) ("fire 任务配置核验：{0}；{1}" -f $taskCred.state, $taskCred.detail)
if ($taskCred.state -ne 'verified') {
    $label = if (-not [string]::IsNullOrWhiteSpace([string]$taskCred.label)) { [string]$taskCred.label }
    elseif ($taskCred.state -eq 'invalid') { 'REFUSED_TASKCONFIG_MISMATCH' }
    else { 'REFUSED_CREDENTIAL_UNVERIFIABLE' }
    Write-WdRefusal -Code 40 -Label $label -Reason ("任务配置核验未通过 → 不进入 FIRED：" + $taskCred.detail) -Extra @{ credentials = $creds } -NetworkActions ([int]$(if ($null -ne $script:credNetworkActions) { $script:credNetworkActions } else { 0 }))
}
if ($HostChecks) {
    # 兼容旧命令行：旧版把 HostChecks 当开关（默认关）；新版 fire 默认强制，故本开关已无副作用。
    Add-WdLog $ctx 'INFO' '-HostChecks 为兼容旧调用方式保留；fire 模式默认强制 HostChecks，本开关无副作用'
}
if ($SkipHostChecks) {
    Add-WdLog $ctx 'WARN' '显式 -SkipHostChecks：④驱动进程 / ⑤到设备 TCP 记为 not_verified，本轮不得判"停净"（退出码 7）'
}
Add-WdLog $ctx 'WARN' ("FIRED 前置条件全部满足：AuthorizedBy={0}；ticket={1}；G6={2}@{3}；G7={4}；清单自身={5}；rev={6}；points={7}；获批配置自身={8}" -f $AuthorizedBy, $AuthzTicket, $G6Signoff, $G6SignedAt, $G7DeployVersion, $(if ($ExpectedManifestSha256) { $ExpectedManifestSha256 } else { '（不适用：非 manifest 形式）' }), $ExpectedTaskRevision, $ExpectedPointCount, $ExpectedApprovedTaskConfigSha256)
Write-Host ("[fire] 授权就绪：by={0} ticket={1} g6={2}@{3} g7={4} rev={5} points={6}" -f $AuthorizedBy, $AuthzTicket, $G6Signoff, $G6SignedAt, $G7DeployVersion, $ExpectedTaskRevision, $ExpectedPointCount)
#endregion

#region ── 五个独立执行流的脚本体 ─────────────────────────────────────────────────
# 说明：每个脚本体都以 param($Ctx) 开头，经 $Body.ToString() 取出源码，在**新的 runspace**里
# 与 $CommonPrelude 拼接后编译执行。runspace 之间只通过 $Ctx 里的并发对象通信。

# ① OBS：观测流。可被 HTTP 阻塞任意久；它的阻塞不影响 TIMER / STOP / MON。
$ObserveBody = {
    param($Ctx)
    $ErrorActionPreference = 'Stop'
    $interval = [int]$Ctx.observeIntervalMs
    $maxMs = [int]$Ctx.observeMaxSec * 1000
    $settleMs = [int]$Ctx.settleMs
    $uri = "$($Ctx.baseUrl)/api/v1/endpoints/$($Ctx.endpointId)"
    $obsStart = Get-WdMono $Ctx
    $seq = 0
    $fail = 0
    $maxReq = 0.0
    $firstStopped = 0.0
    $lastStopMono = 0.0
    $settleStart = 0.0
    $settleEpoch = $null
    $settleSamples = 0
    $settleResets = 0
    $settleOk = $false
    $nextTick = $obsStart
    # 采样连续性（P0-2）：成功样本之间的最大间隔；只有"成功解析出 state"的样本才算数。
    $lastOkMono = 0.0
    $maxGap = 0.0
    Add-WdLog $Ctx 'INFO' "OBS 启动：GET $uri；间隔 $interval ms；观测超时 $($Ctx.observeTimeoutSec) s；总上限 $($Ctx.observeMaxSec) s"
    try {
        while (-not $Ctx.shutdown.Wait(0)) {
            if ((Get-WdMono $Ctx) - $obsStart -ge $maxMs) {
                Add-WdLog $Ctx 'WARN' "OBS 达到观测总上限 $($Ctx.observeMaxSec) s，停止观测"
                break
            }
            $reqSw = [System.Diagnostics.Stopwatch]::StartNew()
            $status = 0
            $err = $null
            $content = $null
            try {
                $resp = Invoke-WebRequest -Uri $uri -Method Get -TimeoutSec ([int]$Ctx.observeTimeoutSec) -SkipHttpErrorCheck -ErrorAction Stop
                $status = [int]$resp.StatusCode
                $content = [string]$resp.Content
            }
            catch {
                $err = $_.Exception.Message
            }
            $reqMs = [math]::Round($reqSw.Elapsed.TotalMilliseconds, 1)
            $mono = Get-WdMono $Ctx
            $seq++

            $state = $null
            $desired = $null
            $epoch = $null
            $detail = $null
            if ($content) {
                try {
                    $o = $content | ConvertFrom-Json -ErrorAction Stop
                    if ($o.PSObject.Properties['runtime']) {
                        if ($o.runtime.PSObject.Properties['state']) { $state = [string]$o.runtime.state }
                        if ($null -ne $o.runtime.epoch) { $epoch = [string]$o.runtime.epoch }
                        if ($null -ne $o.runtime.detail) { $detail = [string]$o.runtime.detail }
                    }
                    if ($o.PSObject.Properties['desired_running']) { $desired = [bool]$o.desired_running }
                    # 端点 connection / driver_id 只在首条成功样本里取一次（供"停干净"④⑤本机核对用）。
                    if ($o.PSObject.Properties['driver_id'] -and -not $Ctx.d['driverId']) {
                        $Ctx.d['driverId'] = [string]$o.driver_id
                    }
                    if ($o.PSObject.Properties['connection'] -and [int]$Ctx.d['devicePort'] -eq 0) {
                        if ($o.connection.PSObject.Properties['host']) { $Ctx.d['deviceHost'] = [string]$o.connection.host }
                        if ($o.connection.PSObject.Properties['port']) { $Ctx.d['devicePort'] = [int]$o.connection.port }
                    }
                }
                catch {
                    if (-not $err) { $err = "JSON 解析失败：$($_.Exception.Message)" }
                }
            }

            $t0 = [double]$Ctx.d['t0Mono']
            $rel = $null
            if ($t0 -gt 0) { $rel = [math]::Round($mono - $t0, 1) }
            $rec = [ordered]@{
                seq             = $seq
                t_local         = (Get-Date).ToString('o')
                t_unix_ns       = ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000000)
                t_mono_ms       = [math]::Round($mono, 1)
                t_rel_ms        = $rel
                state           = $state
                desired_running = $desired
                epoch           = $epoch
                detail          = $detail
                http_status     = $status
                req_ms          = $reqMs
                err             = $err
            }
            Write-WdJsonl $Ctx $Ctx.statesPath $rec
            $Ctx.history.Enqueue($rec)

            # ── 采样连续性：记录"成功样本"之间的最大间隔，供事后判采样是否有效。
            # 失败样本（超时/连不上/JSON 坏）不算成功，因此断流会体现为间隔变大。
            if (-not $err -and $status -eq 200 -and $state -and $t0 -gt 0 -and $mono -ge $t0) {
                $gapBase = if ($lastOkMono -gt 0) { $lastOkMono } else { $t0 }
                $gap = $mono - $gapBase
                if ($gap -gt $maxGap) { $maxGap = $gap }
                $lastOkMono = $mono
                $Ctx.d['lastSuccessMono'] = $mono
            }

            if ($err) { $fail++ }
            if ($reqMs -gt $maxReq) { $maxReq = $reqMs }
            # 观测请求被长时间阻塞本身就是 M1 的证据；记一条（不刷屏：只记 >2 s 的）。
            if ($reqMs -gt 2000) {
                Add-WdLog $Ctx 'WARN' "OBS 单次 GET 耗时 $reqMs ms（观测被阻塞；计时与 Stop 走独立执行流，不受影响）"
            }
            if ($err -and ($fail -eq 1 -or ($fail % 50) -eq 0)) {
                Add-WdLog $Ctx 'WARN' "OBS 第 $fail 次观测失败：$err"
            }

            # ── ⑥ 稳定窗口：只在"Stop 已发出"之后计时，避免把 Start 之前的 STOPPED 当成"停干净"。
            # 每发出一次新的 Stop（竞态补发、或提前触发之后的截止 Stop），窗口都必须**重新计时**：
            # "停干净"只能相对**最后一次** Stop 判定。
            $stopLast = [double]$Ctx.d['stopLastMono']
            if ($stopLast -gt $lastStopMono) {
                $lastStopMono = $stopLast
                if ($firstStopped -gt 0) {
                    Add-WdLog $Ctx 'INFO' 'OBS 观测到新的 Stop 已发出，稳定窗口重新计时（相对最后一次 Stop 判定）'
                }
                $firstStopped = 0.0
                $settleStart = 0.0
                $settleEpoch = $null
                $settleSamples = 0
                $settleOk = $false
            }
            $stopFirst = [double]$Ctx.d['stopFirstMono']
            if ($stopFirst -gt 0 -and $mono -ge $stopFirst) {
                if ($state -eq 'STOPPED' -and $desired -eq $false) {
                    if ($firstStopped -le 0) {
                        $firstStopped = $mono
                        $settleStart = $mono
                        $settleEpoch = $epoch
                        $settleSamples = 1
                        $Ctx.d['obsFirstStoppedMono'] = $mono
                        Add-WdLog $Ctx 'INFO' ("OBS 首次观测到 STOPPED + desired_running=false（mono={0:N1} ms，相对 T0={1:N1} ms），开始 {2} s 稳定窗口" -f $mono, ($mono - $t0), ($settleMs / 1000))
                    }
                    else {
                        if ($settleEpoch -and $epoch -and $epoch -ne $settleEpoch) {
                            $settleResets++
                            $settleStart = $mono
                            $settleEpoch = $epoch
                            $settleSamples = 1
                            Add-WdLog $Ctx 'WARN' "OBS 稳定窗口内 epoch 变化（$settleEpoch -> $epoch），窗口重新计时（第 $settleResets 次重置）"
                        }
                        else {
                            $settleSamples++
                        }
                    }
                }
                elseif ($state) {
                    # 已进入稳定窗口却又看到非 STOPPED / RECONNECTING ⇒ 窗口作废，重新计时。
                    if ($firstStopped -gt 0) {
                        $settleResets++
                        $settleStart = $mono
                        $settleSamples = 0
                        Add-WdLog $Ctx 'WARN' "OBS 稳定窗口被打破：state=$state（第 $settleResets 次重置）"
                    }
                    if ($state -eq 'RECONNECTING') {
                        Add-WdLog $Ctx 'WARN' "OBS 观测到 RECONNECTING（相对 T0=$([math]::Round($mono - $t0, 1)) ms）"
                    }
                }
                # 观测不早于截止结束：即使提前触发（S1/S2/S3/OBS_STALL）已停住，也要等到 T0+30 s
                # 的截止 Stop 发出并复核完才收工——否则"提前停住后又在 30 s 前被迟到 Start 拉起"会被漏掉。
                $deadlineMonoObs = [double]$Ctx.d['deadlineMono']
                if ($firstStopped -gt 0 -and ($mono - $settleStart) -ge $settleMs -and $mono -ge $deadlineMonoObs) {
                    $settleOk = $true
                    Add-WdLog $Ctx 'INFO' ("OBS 稳定窗口完成：{0} s 内无新 epoch、无 RECONNECTING（窗口内样本 {1} 条，重置 {2} 次）" -f ($settleMs / 1000), $settleSamples, $settleResets)
                    break
                }
            }

            # 节拍：按固定 200 ms 对齐；若本次请求已经超时很久，则不再补睡。
            $nextTick += $interval
            $now = Get-WdMono $Ctx
            if ($now -lt $nextTick) {
                [void]$Ctx.wake.Wait([int][math]::Ceiling($nextTick - $now))
            }
            else {
                $nextTick = $now
            }
        }
    }
    finally {
        # 收尾时把"最后一个成功样本 → 现在"也计入最大间隔，避免"断流发生在末尾"被漏判。
        $t0End = [double]$Ctx.d['t0Mono']
        if ($t0End -gt 0) {
            $gapBaseEnd = if ($lastOkMono -gt 0) { $lastOkMono } else { $t0End }
            $tailGap = (Get-WdMono $Ctx) - $gapBaseEnd
            if ($tailGap -gt $maxGap) { $maxGap = $tailGap }
        }
        $Ctx.d['maxSuccessGapMs'] = [math]::Round($maxGap, 1)
        $Ctx.d['settleOk'] = $settleOk
        $Ctx.d['settleSamples'] = $settleSamples
        $Ctx.d['settleResets'] = $settleResets
        $Ctx.d['obsSamples'] = $seq
        $Ctx.d['obsFailures'] = $fail
        $Ctx.d['obsMaxReqMs'] = $maxReq
        # obsExited=true 表示"正常跑完并自己收尾"；MON 用它区分"正常退出"与"异常退出"。
        $Ctx.d['obsExited'] = $true
        $Ctx.obsDone.Set()
        Add-WdLog $Ctx 'INFO' "OBS 退出：样本 $seq 条，失败 $fail 次，单次最长 $maxReq ms，成功样本最大间隔 $([math]::Round($maxGap, 1)) ms，settleOk=$settleOk"
    }
}

# ② TIMER：计时流。**只读单调时钟**，不读端点状态、不发 HTTP、不等任何请求。
$TimerBody = {
    param($Ctx)
    $ErrorActionPreference = 'Stop'
    if (-not $Ctx.armed.Wait(180000)) {
        Add-WdLog $Ctx 'ERROR' 'TIMER 未等到 T0（armed 超时），不发出截止 Stop'
        return
    }
    $deadline = [double]$Ctx.d['deadlineMono']
    Add-WdLog $Ctx 'INFO' ("TIMER 已武装：deadline_mono_ms={0:N1}（= T0 + {1} ms）" -f $deadline, [int]$Ctx.deadlineOffsetMs)
    while (-not $Ctx.shutdown.Wait(0)) {
        $remain = $deadline - (Get-WdMono $Ctx)
        if ($remain -le 0) { break }
        if ($remain -le 25) {
            # 最后 25 ms 自旋：ManualResetEventSlim.Wait 受系统时钟粒度（约 15.6 ms）影响，
            # 自旋把"到达截止 → 入队 Stop"的偏差压到 1 ms 量级（§4.6.1 要求偏差 ≤1 s）。
            while ((Get-WdMono $Ctx) -lt $deadline) { [void][System.Threading.Thread]::Yield() }
            break
        }
        # 分片等待（≤200 ms）以便 shutdown 时能及时退出；并且**留出 20 ms 余量**，
        # 保证任何一次 Wait 的粒度抖动都不会跨过截止点（跨过就轮不到上面的自旋了）。
        $chunk = [math]::Min($remain - 20.0, 200.0)
        if ($chunk -lt 1) { $chunk = 1 }
        [void]$Ctx.wake.Wait([int][math]::Ceiling($chunk))
    }
    $fire = Get-WdMono $Ctx
    $Ctx.d['deadlineFireMono'] = $fire
    Add-WdLog $Ctx 'WARN' ("TIMER 到达截止：mono={0:N1} ms，deadline={1:N1} ms，偏差 {2:N1} ms → 入队 DEADLINE（不等任何 HTTP 完成）" -f $fire, $deadline, ($fire - $deadline))
    $Ctx.stopQueue.Enqueue('DEADLINE')
    [void]$Ctx.stopSignal.Release()
}

# ③ STOP：停止流。**发出** Stop 请求；Stop 自身可能挂 75–80 s，不阻塞观测与计时。
$StopBody = {
    param($Ctx)
    $ErrorActionPreference = 'Stop'
    $uri = "$($Ctx.baseUrl)/api/v1/endpoints/$($Ctx.endpointId)/stop"
    $n = 0
    $lastReasonAt = @{}
    Add-WdLog $Ctx 'INFO' 'STOP 执行流就绪（等信号；不轮询端点状态、不参与计时）'
    while (-not $Ctx.shutdown.Wait(0)) {
        if (-not $Ctx.stopSignal.Wait(200)) { continue }
        # 把当前队列里的原因合并成**一次** Stop；DEADLINE 永不因去重被丢掉。
        $reasons = [System.Collections.Generic.List[string]]::new()
        $r = $null
        while ($Ctx.stopQueue.TryDequeue([ref]$r)) {
            $now = Get-WdMono $Ctx
            $prev = 0.0
            if ($lastReasonAt.ContainsKey($r)) { $prev = [double]$lastReasonAt[$r] }
            if ($r -ne 'DEADLINE' -and $r -ne 'DEADLINE_FAILSAFE' -and $prev -gt 0 -and ($now - $prev) -lt 2000) {
                Add-WdLog $Ctx 'INFO' "STOP 去重：原因 $r 在 2 s 内已触发过，忽略本次重复"
                $r = $null
                continue
            }
            $lastReasonAt[$r] = $now
            if (-not $reasons.Contains($r)) { $reasons.Add($r) }
            $r = $null
        }
        if ($reasons.Count -eq 0) { continue }
        if ($n -ge [int]$Ctx.maxStops) {
            Add-WdLog $Ctx 'WARN' "STOP 次数已达上限 $($Ctx.maxStops) 次，忽略原因：$($reasons -join ',')"
            continue
        }

        $n++
        $tDispatch = Get-Date
        $mono = Get-WdMono $Ctx
        $deadline = [double]$Ctx.d['deadlineMono']
        $deviation = $null
        if ($deadline -gt 0) { $deviation = [math]::Round($mono - $deadline, 1) }
        $Ctx.d['stopInFlight'] = $true
        $Ctx.d['stopCount'] = $n
        if ($n -eq 1) { $Ctx.d['stopFirstMono'] = $mono }
        $Ctx.d['stopLastMono'] = $mono

        $swReq = [System.Diagnostics.Stopwatch]::StartNew()
        $status = 0
        $err = $null
        $body = ''
        try {
            $resp = Invoke-WebRequest -Uri $uri -Method Post -TimeoutSec ([int]$Ctx.stopTimeoutSec) -SkipHttpErrorCheck -ErrorAction Stop
            $status = [int]$resp.StatusCode
            $body = [string]$resp.Content
        }
        catch {
            $err = $_.Exception.Message
        }
        $elapsed = [math]::Round($swReq.Elapsed.TotalMilliseconds, 1)
        $Ctx.d['stopInFlight'] = $false

        $rec = [ordered]@{
            n                   = $n
            reasons             = @($reasons)
            t_dispatch_local    = $tDispatch.ToString('o')
            t_dispatch_unix_ns  = ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000000)
            t_dispatch_mono_ms  = [math]::Round($mono, 1)
            t0_mono_ms          = [double]$Ctx.d['t0Mono']
            deadline_mono_ms    = $deadline
            deviation_ms        = $deviation
            http_status         = $status
            elapsed_ms          = $elapsed
            body                = $body
            err                 = $err
        }
        Write-WdJsonl $Ctx $Ctx.stopsPath $rec
        $Ctx.stopRecords.Enqueue($rec)
        Add-WdLog $Ctx 'WARN' ("STOP #{0} 发出：原因={1}；t_mono={2:N1} ms；deadline={3:N1} ms；偏差={4:N1} ms；http={5}；请求耗时={6} ms{7}" -f $n, ($reasons -join '+'), $mono, $deadline, $deviation, $status, $elapsed, $(if ($err) { "；错误=$err" } else { '' }))
        if ($reasons -contains 'RACE_START_AFTER_STOP') {
            Add-WdLog $Ctx 'WARN' 'RACE_START_AFTER_STOP：Stop 之后观测到 RUNNING 或 epoch 变化，已再次 Stop（§5.3）'
        }
    }
}

# ④ START：启动流。发 Start 一次、不重发；超时只记录，不影响计时。
$StartBody = {
    param($Ctx)
    $ErrorActionPreference = 'Stop'
    if (-not $Ctx.startGo.Wait(180000)) {
        Add-WdLog $Ctx 'ERROR' 'START 未收到放行信号，未发出 Start'
        return
    }
    $uri = "$($Ctx.baseUrl)/api/v1/endpoints/$($Ctx.endpointId)/start"
    $tDispatch = Get-Date
    $mono = Get-WdMono $Ctx
    $Ctx.d['startDispatchMono'] = $mono
    $Ctx.d['startDispatchLocal'] = $tDispatch.ToString('o')
    $swReq = [System.Diagnostics.Stopwatch]::StartNew()
    $status = 0
    $err = ''
    $body = ''
    $outcome = 'unknown'
    try {
        $resp = Invoke-WebRequest -Uri $uri -Method Post -TimeoutSec ([int]$Ctx.startTimeoutSec) -SkipHttpErrorCheck -ErrorAction Stop
        $status = [int]$resp.StatusCode
        $body = [string]$resp.Content
        $outcome = 'returned'
    }
    catch {
        $err = $_.Exception.Message
        if ($err -match '(?i)timeout|timed out|cancel|abort') { $outcome = 'timeout' } else { $outcome = 'error' }
    }
    $elapsed = [math]::Round($swReq.Elapsed.TotalMilliseconds, 1)
    $Ctx.d['startOutcome'] = $outcome
    $Ctx.d['startStatus'] = $status
    $Ctx.d['startErr'] = $err
    $Ctx.d['startElapsedMs'] = $elapsed
    $Ctx.d['startBody'] = $body
    Add-WdLog $Ctx 'WARN' ("START 结果={0}（http={1}，耗时={2} ms）{3}；截止计时与 Start 返回值无关（§5.1-3/4）" -f $outcome, $status, $elapsed, $(if ($err) { "；错误=$err" } else { '' }))
    if ($outcome -ne 'returned' -or $status -ne 200) {
        Add-WdLog $Ctx 'WARN' 'START 结果不确定（在途/失败）：按 §5.3 在途 Start 规则，不得据此判 PASS；截止 Stop 仍按 T0+30 s 发出'
    }
}

# ⑤ MON：监护流（P0-2 观测心跳 + M10 工作流存活）。**不读端点、不发 HTTP、只管两件事**。
$MonitorBody = {
    param($Ctx)
    $ErrorActionPreference = 'Stop'
    $interval = [int]$Ctx.monitorIntervalMs
    $stallMs = [int]$Ctx.observeStallSec * 1000
    $watched = @($Ctx.flowHandles | Where-Object { $_.Name -ne 'mon' })
    $reported = @{}
    $failsafeAt = 0.0
    Add-WdLog $Ctx 'INFO' ("MON 启动：心跳阈值 {0} ms，监护 {1}" -f $stallMs, (($watched | ForEach-Object { $_.Name }) -join ','))

    function Test-WdExpectedExit {
        param($Ctx, [string]$Name)
        # 每类工作流的"正常完成"条件。判定不了就按异常处理（宁严不松）。
        switch ($Name) {
            'obs' { return [bool]$Ctx.d['obsExited'] }
            'timer' { return ([double]$Ctx.d['deadlineFireMono'] -gt 0) }
            'start' { return ([double]$Ctx.d['startDispatchMono'] -gt 0) }
            'stop' { return $false }
            default { return $false }
        }
    }

    while (-not $Ctx.shutdown.Wait(0)) {
        $Ctx.d['monTicks'] = [int]$Ctx.d['monTicks'] + 1
        $now = Get-WdMono $Ctx
        $t0 = [double]$Ctx.d['t0Mono']

        # ── A) 观测心跳：距最近一次成功样本超阈值 ⇒ 观测通道失效，提前 Stop
        # obsExited=true 表示 OBS 已**正常跑完并退出**（例如稳定窗口完成）：此时不会有新样本，
        # 不能把它误判为"断流"——否则会在收尾阶段凭空多打一次 OBS_STALL。
        if ($stallMs -gt 0 -and $t0 -gt 0 -and -not [bool]$Ctx.d['obsExited'] -and -not [bool]$Ctx.d['obsStallFired']) {
            $lastOk = [double]$Ctx.d['lastSuccessMono']
            $base = [math]::Max($lastOk, $t0)
            if (($now - $base) -ge $stallMs) {
                $Ctx.d['obsStallFired'] = $true
                $missed = [int][math]::Floor(($now - $base) / [double]$Ctx.observeIntervalMs)
                $ev = [ordered]@{
                    kind        = 'OBS_STALL'
                    worker      = 'obs'
                    t_mono_ms   = [math]::Round($now, 1)
                    t_rel_ms    = [math]::Round($now - $t0, 1)
                    gap_ms      = [math]::Round($now - $base, 1)
                    threshold_ms = $stallMs
                    missed_ticks = $missed
                    last_success_mono_ms = [math]::Round($lastOk, 1)
                }
                $Ctx.monEvents.Enqueue($ev)
                Add-WdLog $Ctx 'ERROR' ("MON 观测心跳超时：距最近一次成功样本 {0:N0} ms（阈值 {1} ms，连续 {2} 个 200 ms 心跳未更新，相对 T0={3:N1} ms）→ 判观测通道失效，提前 Stop（不等被阻塞的 GET 返回）" -f ($now - $base), $stallMs, $missed, ($now - $t0))
                $Ctx.stopQueue.Enqueue('OBS_STALL')
                [void]$Ctx.stopSignal.Release()
            }
        }

        # ── B) 工作流异常退出：不静默继续（M10）
        foreach ($f in $watched) {
            if ($f.Name -eq 'mon' -or $reported.ContainsKey($f.Name)) { continue }
            $done = $false
            try { $done = [bool]$f.Handle.IsCompleted } catch { $done = $true }
            if (-not $done) { continue }
            if (Test-WdExpectedExit $Ctx $f.Name) { continue }
            $reported[$f.Name] = $true
            $inv = ''
            $reason = ''
            try { $inv = [string]$f.PS.InvocationStateInfo.State } catch { $inv = 'unknown' }
            try {
                if ($f.PS.InvocationStateInfo.Reason) { $reason = [string]$f.PS.InvocationStateInfo.Reason.Message }
            }
            catch { }
            $Ctx.d['workerAbortCount'] = [int]$Ctx.d['workerAbortCount'] + 1
            $list = @($Ctx.d['workersAborted']); $list += $f.Name
            $Ctx.d['workersAborted'] = $list
            $ev = [ordered]@{
                kind             = 'WORKER_ABORT'
                worker           = $f.Name
                invocation_state = $inv
                reason           = $reason
                t_mono_ms        = [math]::Round($now, 1)
                t_rel_ms         = [math]::Round($now - $t0, 1)
            }
            $Ctx.monEvents.Enqueue($ev)
            Add-WdLog $Ctx 'ERROR' ("MON 工作流异常退出：'{0}'（state={1}，原因={2}，相对 T0={3:N1} ms）→ 本轮作废，立即 Stop 并交人工处置" -f $f.Name, $inv, $reason, ($now - $t0))
            if ($f.Name -eq 'obs') {
                $Ctx.d['obsAborted'] = $true
                # 观测流已死：放行协调者收尾，避免主流程白等到 ObserveMaxSec+60
                $Ctx.obsDone.Set()
            }
            if ($f.Name -eq 'timer') {
                $Ctx.d['timerDead'] = $true
                $failsafeAt = $now
            }
            $Ctx.stopQueue.Enqueue('WORKER_ABORT')
            [void]$Ctx.stopSignal.Release()
        }

        # ── C) 截止调度接管：TIMER 死了也必须保证 T0+30 s 仍有 Stop（F4：它是调度目标）
        if ([bool]$Ctx.d['timerDead'] -and -not [bool]$Ctx.d['deadlineFailsafeFired']) {
            $deadline = [double]$Ctx.d['deadlineMono']
            if ($deadline -gt 0 -and $now -ge $deadline) {
                $Ctx.d['deadlineFailsafeFired'] = $true
                $Ctx.d['deadlineFireMono'] = $now
                Add-WdLog $Ctx 'ERROR' ("MON 接管截止调度：TIMER 已失效，mono={0:N1} ms 到达 deadline={1:N1} ms（偏差 {2:N1} ms）→ 入队 DEADLINE_FAILSAFE" -f $now, $deadline, ($now - $deadline))
                $Ctx.stopQueue.Enqueue('DEADLINE_FAILSAFE')
                [void]$Ctx.stopSignal.Release()
            }
        }

        [void]$Ctx.shutdown.Wait($interval)
    }
}
#endregion

#region ── 启动执行流（全部在 FIRED 之前就绪）────────────────────────────────────
function Start-WdFlow {
    param([string]$Name, [string]$Body, [hashtable]$Ctx)
    # 注意：param() 必须是脚本的第一条语句，所以共享函数放在 param 之后；
    # 脚本体本身（含自己的 param($Ctx)）作为内层 scriptblock 调用，$Ctx 由外层传入。
    $code = "param(`$Ctx)`n" + $script:CommonPrelude + "`n& {`n" + $Body + "`n} `$Ctx"
    $rs = [runspacefactory]::CreateRunspace()
    $rs.Name = "wd-$Name"
    $rs.Open()
    $ps = [powershell]::Create()
    $ps.Runspace = $rs
    [void]$ps.AddScript($code)
    [void]$ps.AddArgument($Ctx)
    $handle = $ps.BeginInvoke()
    return [pscustomobject]@{ Name = $Name; PS = $ps; RS = $rs; Handle = $handle }
}

$ctx.deadlineOffsetMs = $script:DEADLINE_OFFSET_MS
$flows = @()
$flows += Start-WdFlow -Name 'obs' -Body $ObserveBody.ToString() -Ctx $ctx
$flows += Start-WdFlow -Name 'timer' -Body $TimerBody.ToString() -Ctx $ctx
$flows += Start-WdFlow -Name 'stop' -Body $StopBody.ToString() -Ctx $ctx
$flows += Start-WdFlow -Name 'start' -Body $StartBody.ToString() -Ctx $ctx
# MON 需要拿到其它流的句柄，所以最后启动（句柄表在启动 MON 之前发布到 $ctx）。
$ctx.flowHandles = @($flows)
$flows += Start-WdFlow -Name 'mon' -Body $MonitorBody.ToString() -Ctx $ctx
Add-WdLog $ctx 'INFO' "五个独立执行流已启动：$((($flows | ForEach-Object { $_.Name }) -join ', '))"

# 观测预热：让 OBS 先跑起来，使 watchdog-states.jsonl 覆盖 T0-1.5 s（§5.1-5）。
# 注意：这只是"观测先启动"，T0 仍由随后的 FIRED 步骤生成，预热不参与任何计时；
# 且预热只在 fire 模式（已通过显式授权）执行——arm 分支永远不会走到这里。
Start-Sleep -Milliseconds 1500
#endregion

#region ── FIRED：由 watchdog 自己取 T0 并立即发出 Start（G6-FIRED / §5.1-2/3）────
# 顺序（同一段代码内完成，中间不插入任何等待）：
#   取单调时刻 → 发布 T0/截止（武装 TIMER）→ 写 t0.stamp → 放行 START 流。
$t0Mono = Get-WdMono $ctx
$t0Dto = [DateTimeOffset]::Now
$t0UnixNs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000000
$deadlineMono = $t0Mono + $script:DEADLINE_OFFSET_MS
$deadlineDto = $t0Dto.AddMilliseconds($script:DEADLINE_OFFSET_MS)
$ctx.d['t0Mono'] = $t0Mono
$ctx.d['deadlineMono'] = $deadlineMono
$ctx.d['t0UnixNs'] = $t0UnixNs
$ctx.armed.Set()

$stamp = [ordered]@{
    t0_local            = $t0Dto.ToString('o')
    t0_unix_ns          = $t0UnixNs
    t0_mono_ms          = [math]::Round($t0Mono, 1)
    deadline_local      = $deadlineDto.ToString('o')
    deadline_unix_ns    = $t0UnixNs + ([int64]$script:DEADLINE_OFFSET_MS * 1000000)
    deadline_mono_ms    = [math]::Round($deadlineMono, 1)
    offset_sec          = $script:DEADLINE_OFFSET_MS / 1000
    deadline_semantics  = 'Stop 请求的截止调度目标（不保证驱动进程/设备通信已在第 30 s 物理终止）'
    clock               = 'System.Diagnostics.Stopwatch（单调）'
    generated_by        = "stop-watchdog.ps1 PID=$PID（FIRED 时刻自生成，不接受外部指定）"
    endpoint_id         = $EndpointId
    base_url            = $ctx.baseUrl
    mode                = $Mode
    credentials         = [ordered]@{
        overall      = $creds.overall
        authorized_by = $AuthorizedBy
        authz_ticket = $AuthzTicket
        g6_signoff   = $G6Signoff
        g6_signed_at = $G6SignedAt
        g7_deploy_version = $G7DeployVersion
        g7_binary_path    = $G7BinaryPath
        expected_manifest_sha256 = $ExpectedManifestSha256
        expected_revision = $ExpectedTaskRevision
        # 语义：获批任务配置的点数（BLOCKED-01 起；不是 diagnostics.point_count）。
        expected_task_point_count = $ExpectedPointCount
        approved_task_config = $ApprovedTaskConfig
        expected_tasks_fingerprint = $ExpectedTaskFingerprint
        expected_approved_task_config_sha256 = $ExpectedApprovedTaskConfigSha256
        injection_guard   = $injectionGuard
        items        = $creds.items
    }
}
[System.IO.File]::WriteAllText($t0Path, ($stamp | ConvertTo-Json -Depth 4), [System.Text.UTF8Encoding]::new($false))
Add-WdLog $ctx 'WARN' ("FIRED：T0 = {0}（mono {1:N1} ms），截止 = T0+{2} ms = {3}（mono {4:N1} ms）" -f $t0Dto.ToString('o'), $t0Mono, $script:DEADLINE_OFFSET_MS, $deadlineDto.ToString('o'), $deadlineMono)

$ctx.startGo.Set()
$t0ToStart = (Get-WdMono $ctx) - $t0Mono
Add-WdLog $ctx 'INFO' ("已放行 START 流：距 T0 {0:N1} ms（该值即取 T0 到发出 Start 之间的实际间隔，供事后核对）" -f $t0ToStart)
Write-Host ("[FIRED] T0={0}  deadline={1}  (mono {2:N1} / {3:N1} ms)" -f $t0Dto.ToString('HH:mm:ss.fff'), $deadlineDto.ToString('HH:mm:ss.fff'), $t0Mono, $deadlineMono)
#endregion

#region ── 协调者主循环：读样本 → 判定触发 → 入队（自己从不发 HTTP）──────────────
$preStartDetail = ''
$preStartEpoch = ''
$sawPreStartSample = $false
$runningEpoch = $null
$reconStart = 0.0
$raceCount = 0
$lastRaceAt = 0.0
$lastValid = $null
$lastMonTicks = 0
$lastMonTickMono = Get-WdMono $ctx
$abortInjected = $false
# MON 事件累计表：主流程边消费边累计（队列会被消费掉，summary 需要完整列表）。
$allMonEvents = [System.Collections.Generic.List[object]]::new()

function Send-WdStopRequest {
    param([string]$Reason)
    # DEADLINE / DEADLINE_FAILSAFE 属于调度目标；RACE 是竞态处置；OBS_STALL / WORKER_ABORT
    # 各自有独立判定轴（采样轴 / 工作流轴），都不算"S1/S2/S3/S6 提前触发"。
    if ($Reason -notin @('DEADLINE', 'DEADLINE_FAILSAFE', 'RACE_START_AFTER_STOP', 'OBS_STALL', 'WORKER_ABORT')) {
        $ctx.d['earlyTriggered'] = $true
    }
    $ctx.stopQueue.Enqueue($Reason)
    [void]$ctx.stopSignal.Release()
}

$coordStart = Get-WdMono $ctx
$loopDone = $false
while (-not $loopDone) {
    $item = $null
    while ($ctx.history.TryDequeue([ref]$item)) {
        $s = $item
        $item = $null
        $mono = [double]$s.t_mono_ms
        $state = [string]$s.state
        $epoch = [string]$s.epoch
        $detail = [string]$s.detail
        $desired = $s.desired_running
        if ($state) { $lastValid = $s }

        $t0 = [double]$ctx.d['t0Mono']
        if ($t0 -le 0 -or $mono -lt $t0) {
            # Start 之前的样本：只用来记录"基线 detail/epoch"，不参与触发判定。
            if (-not $sawPreStartSample -and $state) {
                $sawPreStartSample = $true
                $preStartDetail = $detail
                $preStartEpoch = $epoch
                Add-WdLog $ctx 'INFO' "Start 前基线：state=$state desired=$desired epoch=$epoch detail=$(if ($detail) { $detail } else { '（空）' })"
            }
            continue
        }

        $stopFirst = [double]$ctx.d['stopFirstMono']

        # ── Stop 发出之后：竞态检测（§5.3）──────────────────────────────────────
        if ($stopFirst -gt 0 -and $mono -ge $stopFirst) {
            $postBase = [string]$ctx.d['postStopEpoch']
            if ($state -and $state -ne 'STOPPED' -and $postBase -eq '') { $ctx.d['postStopEpoch'] = $epoch }
            $isRace = $false
            if ($state -eq 'RUNNING') { $isRace = $true }
            elseif ($postBase -ne '' -and $epoch -and $epoch -ne $postBase) { $isRace = $true }
            if ($isRace -and $raceCount -lt $script:MAX_STOPS) {
                if (($mono - $lastRaceAt) -ge 2000 -or $lastRaceAt -le 0) {
                    $raceCount++
                    $lastRaceAt = $mono
                    $ctx.d['raceCount'] = $raceCount
                    Add-WdLog $ctx 'WARN' ("竞态：Stop 之后观测到 state={0} epoch={1}（相对 T0={2:N1} ms，desired={3}）→ 记录 RACE_START_AFTER_STOP 并再次 Stop（第 {4} 次）" -f $state, $epoch, ($mono - $t0), $desired, $raceCount)
                    Send-WdStopRequest -Reason 'RACE_START_AFTER_STOP'
                }
            }
            continue
        }

        # ── Stop 发出之前：S1/S2/S3/S6 触发判定（首次确认即停，无"连续 N 次"宽限）──
        if ($state -eq 'FAILED') {
            Add-WdLog $ctx 'ERROR' ("触发 S6：state=FAILED（相对 T0={0:N1} ms，detail={1}）→ 立即 Stop" -f ($mono - $t0), $detail)
            Send-WdStopRequest -Reason 'S6_FAILED'
            continue
        }

        # S1/S2：detail 里的通信失败。只认 Start 之后**新出现/变化**的 detail——
        # Start 之前端点可能残留上一轮的失败 detail，那不是"本轮首次确认的通信失败"。
        if ($detail -and $detail -ne $preStartDetail -and $detail -match $script:COMM_FAIL_PATTERN) {
            $clause = if ($detail -match '(?i)S7_JOB_') { 'S1' } else { 'S2' }
            Add-WdLog $ctx 'ERROR' ("触发 {0}：首次确认通信失败 detail={1}（相对 T0={2:N1} ms，state={3}）→ 立即 Stop（不设连续 N 次宽限）" -f $clause, $detail, ($mono - $t0), $state)
            Send-WdStopRequest -Reason "${clause}_COMM_FAIL"
            continue
        }

        # S3：除 Start 那一次外 epoch 再变；或 RECONNECTING ≥10 s。
        if ($state -eq 'RUNNING' -and $epoch) {
            if ($null -eq $runningEpoch) {
                $runningEpoch = $epoch
                Add-WdLog $ctx 'INFO' "首个 RUNNING 样本：epoch=$epoch（相对 T0=$([math]::Round($mono - $t0, 1)) ms）"
            }
            elseif ($epoch -ne $runningEpoch) {
                Add-WdLog $ctx 'ERROR' ("触发 S3：RUNNING 期间 epoch 变化（{0} -> {1}，相对 T0={2:N1} ms）→ 立即 Stop" -f $runningEpoch, $epoch, ($mono - $t0))
                Send-WdStopRequest -Reason 'S3_EPOCH_CHANGE'
                continue
            }
        }
        if ($state -eq 'RECONNECTING') {
            if ($reconStart -le 0) { $reconStart = $mono }
            elseif (($mono - $reconStart) -ge 10000) {
                Add-WdLog $ctx 'ERROR' ("触发 S3：RECONNECTING 已持续 {0:N1} ms（相对 T0={1:N1} ms）→ 立即 Stop" -f ($mono - $reconStart), ($mono - $t0))
                Send-WdStopRequest -Reason 'S3_RECONNECTING_10S'
                $reconStart = 0.0
                continue
            }
        }
        else {
            $reconStart = 0.0
        }
    }

    # 把新产生的 Stop 记录打到控制台（主流程只读记录，不参与发请求）。
    $sr = $null
    while ($ctx.stopRecords.TryDequeue([ref]$sr)) {
        $dev = $sr.deviation_ms
        $devText = if ($null -ne $dev) { ('{0:+0.0;-0.0;0.0} ms' -f [double]$dev) } else { 'N/A' }
        Write-Host ("[STOP #{0}] {1}  reasons={2}  deviation={3}  http={4}  elapsed={5} ms" -f $sr.n, $sr.t_dispatch_local, (($sr.reasons) -join '+'), $devText, $sr.http_status, $sr.elapsed_ms)
    }

    # 把 MON 事件打到控制台（心跳失效 / 工作流异常），并累计到 $allMonEvents 供 summary 落盘。
    $me = $null
    while ($ctx.monEvents.TryDequeue([ref]$me)) {
        [void]$allMonEvents.Add($me)
        if ($me.kind -eq 'OBS_STALL') {
            Write-Host ("[OBS-STALL] 观测断流 {0:N0} ms > 阈值 {1} ms（连续 {2} 个心跳未更新）→ 提前 Stop；采集判 INVALID" -f $me.gap_ms, $me.threshold_ms, $me.missed_ticks)
        }
        elseif ($me.kind -eq 'WORKER_ABORT') {
            Write-Host ("[WORKER-ABORT] 工作流 '{0}' 异常退出（state={1}，原因={2}）→ 本轮作废，须人工处置" -f $me.worker, $me.invocation_state, $me.reason)
        }
    }

    # ── 观测心跳的**兜底**：正式检测由 MON 流独立负责（P0-2）。主流程只检查 MON 是否还活着；
    # MON 自己挂了也不能让"断流"漏判，此时由主流程接管（并在日志里写明是接管）。
    $ticks = [int]$ctx.d['monTicks']
    if ($ticks -ne $lastMonTicks) { $lastMonTicks = $ticks; $lastMonTickMono = Get-WdMono $ctx }
    if ($ObserveStallSec -gt 0 -and -not [bool]$ctx.d['obsStallFired'] -and -not [bool]$ctx.d['obsExited']) {
        $t0h = [double]$ctx.d['t0Mono']
        if ($t0h -gt 0 -and ((Get-WdMono $ctx) - $lastMonTickMono) -gt 2000) {
            $lastOk = [double]$ctx.d['lastSuccessMono']
            $stallBase = [math]::Max($lastOk, $t0h)
            $nowH = Get-WdMono $ctx
            if (($nowH - $stallBase) -ge ($ObserveStallSec * 1000)) {
                $ctx.d['obsStallFired'] = $true
                Add-WdLog $ctx 'ERROR' ("MON 心跳停摆（>{0} ms 未 tick），主流程接管观测心跳判定：距最近一次成功样本 {1:N0} ms > 阈值 {2} s → 提前 Stop" -f 2000, ($nowH - $stallBase), $ObserveStallSec)
                Write-Host ("[OBS-STALL] （主流程接管）观测断流 {0:N0} ms > 阈值 {1} s → 提前 Stop；采集判 INVALID" -f ($nowH - $stallBase), $ObserveStallSec)
                Send-WdStopRequest -Reason 'OBS_STALL'
            }
        }
    }

    # ── 离线测试注入：强制中止某个工作流，复现 M10（工作线程异常退出）────────────
    if ($TestAbortFlow -and -not $abortInjected) {
        $t0i = [double]$ctx.d['t0Mono']
        if ($t0i -gt 0 -and ((Get-WdMono $ctx) - $t0i) -ge ($TestAbortAfterSec * 1000)) {
            $abortInjected = $true
            $target = @($flows | Where-Object { $_.Name -eq $TestAbortFlow }) | Select-Object -First 1
            if ($target) {
                Add-WdLog $ctx 'WARN' "TEST 注入：强制中止工作流 '$TestAbortFlow'（模拟工作线程异常退出，M10）"
                try { $target.PS.Stop() } catch { Add-WdLog $ctx 'ERROR' "TEST 注入中止失败：$($_.Exception.Message)" }
            }
        }
    }

    # ── 观测总上限的**独立**执行：不依赖 OBS 线程的请求返回。
    $t0c = [double]$ctx.d['t0Mono']
    if ($t0c -gt 0 -and -not [bool]$ctx.d['obsTruncated'] -and ((Get-WdMono $ctx) - $t0c) -ge ($ObserveMaxSec * 1000)) {
        $ctx.d['obsTruncated'] = $true
        Add-WdLog $ctx 'ERROR' "观测总上限 $ObserveMaxSec s 已到（独立计时，不等在途 GET）→ 强制结束观测并收尾；本轮证据不完整"
        Write-Host ''
        Write-Host ("!! 观测已超过 {0} s 上限（可能有 GET 卡死）：本轮证据不完整，现场须先人工核实端点状态再决定处置。" -f $ObserveMaxSec)
        $loopDone = $true
        break
    }

    if ($ctx.obsDone.Wait(0)) {
        # 观测流已退出：若是异常退出（OBS 自己没跑完 finally），MON 会给出 WORKER_ABORT。
        $loopDone = $true
        break
    }
    # 观测流意外退出（例如 runspace 崩了）也要收敛，不能无限等。
    if (((Get-WdMono $ctx) - $coordStart) -gt (($ObserveMaxSec + 60) * 1000)) {
        Add-WdLog $ctx 'ERROR' '协调者等待观测超时（观测流可能已异常退出），强制收敛'
        $loopDone = $true
        break
    }
    [void]$ctx.obsDone.Wait(20)
}
# 收尾：把剩余样本判完（观测流退出后队列里可能还有几条）。
$item = $null
while ($ctx.history.TryDequeue([ref]$item)) {
    if ([string]$item.state) { $lastValid = $item }
    $item = $null
}
$sr = $null
while ($ctx.stopRecords.TryDequeue([ref]$sr)) {
    Write-Host ("[STOP #{0}] {1}  reasons={2}  deviation={3} ms  http={4}  elapsed={5} ms" -f $sr.n, $sr.t_dispatch_local, (($sr.reasons) -join '+'), $sr.deviation_ms, $sr.http_status, $sr.elapsed_ms)
}
$me = $null
while ($ctx.monEvents.TryDequeue([ref]$me)) {
    [void]$allMonEvents.Add($me)
    if ($me.kind -eq 'OBS_STALL') {
        Write-Host ("[OBS-STALL] 观测断流 {0:N0} ms > 阈值 {1} ms（相对 T0 {2:N0} ms）→ 提前 Stop；采集判 INVALID" -f $me.gap_ms, $me.threshold_ms, $me.t_rel_ms)
    }
    elseif ($me.kind -eq 'WORKER_ABORT') {
        Write-Host ("[WORKER-ABORT] 工作流 '{0}' 异常退出（state={1}，原因={2}）→ 本轮作废，须人工处置" -f $me.worker, $me.invocation_state, $me.reason)
    }
}
#endregion

#region ── 等待在途 Stop 落盘（Stop 可能挂 75–80 s，不能把"已发出"当"已完成"）────
$waitStart = Get-WdMono $ctx
# 观测已被强制截断时，不再为了等在途 Stop 再耗掉 120 s：最多再等 10 s，然后如实记录"结果未知"。
$waitCapMs = if ([bool]$ctx.d['obsTruncated']) { 10000 } else { ($StopTimeoutSec + 5) * 1000 }
while ([bool]$ctx.d['stopInFlight'] -and ((Get-WdMono $ctx) - $waitStart) -lt $waitCapMs) {
    Start-Sleep -Milliseconds 50
}
if ([bool]$ctx.d['stopInFlight']) {
    Add-WdLog $ctx 'ERROR' "在途 Stop 未在 $([math]::Round($waitCapMs / 1000, 1)) s 内返回，结果未知（不把'已发出'当'已完成'）"
}
$allStops = @()
$sr = $null
# stopRecords 已被消费过，这里直接读 jsonl 兜底（保证 summary 里有完整记录）。
if (Test-Path $stopsPath) {
    foreach ($line in [System.IO.File]::ReadAllLines($stopsPath)) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        try { $allStops += ($line | ConvertFrom-Json) } catch { }
    }
}
$stopCount = $allStops.Count
#endregion

#region ── "停干净"六项复核（§5.3；缺一不算）+ HostChecks 三态（P0-3）────────────
# 每一项都有三态：converged / not_converged / indeterminate；跳过记为 not_verified。
# **查询失败、权限不足、工具缺失、结果不确定一律 indeterminate** —— 绝不折算成"没有连接"。
function New-WdItem {
    param([string]$State, [string]$Detail, [hashtable]$Extra)
    return [ordered]@{
        required = $true
        state    = $State
        ok       = ($State -eq 'converged')
        detail   = $Detail
        extra    = $Extra
    }
}

function Get-WdProcessTable {
    # 返回 @(@{name;cmdline}) 或 $null（无法查询）。$null 一律按 indeterminate 处理。
    if ($IsWindows) {
        try {
            $procs = @(Get-CimInstance -ClassName Win32_Process -ErrorAction Stop)
            return @($procs | ForEach-Object { @{ name = [string]$_.Name; cmdline = [string]$_.CommandLine } })
        }
        catch { return $null }
    }
    if (Test-Path '/proc/self/cmdline') {
        $list = @()
        try {
            foreach ($pd in [System.IO.Directory]::GetDirectories('/proc')) {
                $pn = [System.IO.Path]::GetFileName($pd)
                if ($pn -notmatch '^\d+$') { continue }
                $cl = $null
                try { $cl = [System.IO.File]::ReadAllText((Join-Path $pd 'cmdline')) } catch { continue }
                if ([string]::IsNullOrWhiteSpace($cl)) { continue }
                $cl = ($cl -replace "`0", ' ').Trim()
                $list += @{ name = ([string]($cl.Split(' ')[0])); cmdline = $cl }
            }
            return $list
        }
        catch { return $null }
    }
    # 其它平台：无法枚举进程命令行 → 判 indeterminate（不是"没有进程"）。
    return $null
}

function Test-WdDriverProcess {
    # ④ 驱动进程应为空；**只认目标驱动**（driver_id），其它 mesa-driver 进程单列 excluded。
    param($Ctx, [bool]$InjectError)
    if ($InjectError) {
        return New-WdItem -State 'indeterminate' -Detail '注入故障（-TestHostProbeError process）：权限不足/查询失败 → 无法确定（不得判停净）' -Extra @{ provider = 'injected' }
    }
    $driverId = [string]$Ctx.d['driverId']
    if (-not $driverId) {
        return New-WdItem -State 'indeterminate' -Detail '未能从端点快照取到 driver_id → 无法确定目标驱动（不得判停净）' -Extra @{ provider = 'none' }
    }
    $table = Get-WdProcessTable
    if ($null -eq $table) {
        return New-WdItem -State 'indeterminate' -Detail '本机无法枚举进程命令行（权限/平台限制）→ 无法确定（不得判停净）' -Extra @{ provider = 'unavailable'; driver_id = $driverId }
    }
    $idLower = $driverId.ToLowerInvariant()
    $targetForms = @(
        ('mesa-driver-' + $idLower),
        ('mesa_driver_' + ($idLower -replace '-', '_'))
    )
    $matched = @()
    $excluded = @()
    foreach ($p in $table) {
        $hay = (([string]$p.name) + ' ' + ([string]$p.cmdline)).ToLowerInvariant()
        if ($hay -notmatch 'mesa[-_]driver') { continue }
        $isTarget = $false
        foreach ($f in $targetForms) { if ($hay.Contains($f)) { $isTarget = $true; break } }
        if (-not $isTarget -and $hay -match ('--driver[-_]id[ ="]?' + [regex]::Escape($idLower))) { $isTarget = $true }
        if ($isTarget) { $matched += ([string]$p.name) } else { $excluded += ([string]$p.name) }
    }
    $excluded = @($excluded | Sort-Object -Unique)
    $extra = @{ driver_id = $driverId; matched = @($matched | Sort-Object -Unique); excluded = $excluded; provider = $(if ($IsWindows) { 'win-cim' } else { 'proc' }) }
    if ($matched.Count -gt 0) {
        return New-WdItem -State 'not_converged' -Detail ("目标驱动 $driverId 进程仍存在：{0}（已按 driver_id 精确匹配；其它 mesa-driver 进程 {1} 条已排除）" -f (($matched | Sort-Object -Unique) -join ','), $excluded.Count) -Extra $extra
    }
    return New-WdItem -State 'converged' -Detail ("未发现目标驱动 $driverId 的进程（mesa-driver-{0}；排除其它 mesa-driver 进程 {1} 条）" -f $driverId, $excluded.Count) -Extra $extra
}

function Get-WdTcpEstablished {
    # 返回 @(@{addr;port}) 的 ESTABLISHED 远端列表，或 $null（无法查询）。
    if ($IsWindows) {
        try {
            $c = @(Get-NetTCPConnection -State Established -ErrorAction Stop)
            return @($c | ForEach-Object { @{ addr = [string]$_.RemoteAddress; port = [int]$_.RemotePort } })
        }
        catch { return $null }
    }
    if ((Test-Path '/proc/net/tcp') -or (Test-Path '/proc/net/tcp6')) {
        $list = @()
        try {
            foreach ($f in @('/proc/net/tcp', '/proc/net/tcp6')) {
                if (-not (Test-Path $f)) { continue }
                $lines = [System.IO.File]::ReadAllLines($f)
                foreach ($ln in ($lines | Select-Object -Skip 1)) {
                    $cols = ($ln -replace '\s+', ' ').Trim().Split(' ')
                    if ($cols.Count -lt 4) { continue }
                    if ($cols[3] -ne '01') { continue }   # 01 = ESTABLISHED
                    $rem = $cols[2]
                    $parts = $rem.Split(':')
                    if ($parts.Count -ne 2) { continue }
                    $hexIp = $parts[0]; $hexPort = $parts[1]
                    $port = [Convert]::ToInt32($hexPort, 16)
                    $addr = 'unknown'
                    if ($hexIp.Length -eq 8) {
                        $b = @()
                        for ($i = 6; $i -ge 0; $i -= 2) { $b += [Convert]::ToInt32($hexIp.Substring($i, 2), 16) }
                        $addr = ($b -join '.')
                    }
                    else {
                        $groups = @()
                        for ($i = 0; $i -lt 32; $i += 8) {
                            $pair = $hexIp.Substring($i, 8)
                            $bytes = @()
                            for ($j = 6; $j -ge 0; $j -= 2) { $bytes += $pair.Substring($j, 2) }
                            $groups += ($bytes -join '')
                        }
                        $addr = (($groups -join ':') -replace '0000:', '')
                        if ($addr -eq '00000000000000000000000001000000') { $addr = '::1' }
                    }
                    $list += @{ addr = $addr; port = $port }
                }
            }
            return $list
        }
        catch { return $null }
    }
    return $null
}

function Test-WdDeviceTcp {
    # ⑤ 到设备的采集连接应为空；**只认目标 host + 目标 port**（现场 3389/RDP 严格区分）。
    param($Ctx, [bool]$InjectError)
    $devHost = [string]$Ctx.d['deviceHost']
    $devPort = [int]$Ctx.d['devicePort']
    if ($InjectError) {
        return New-WdItem -State 'indeterminate' -Detail '注入故障（-TestHostProbeError tcp）：权限不足/查询失败 → 无法确定（不得判停净）' -Extra @{ provider = 'injected' }
    }
    if (-not $devHost -or $devPort -le 0) {
        return New-WdItem -State 'indeterminate' -Detail '未能从端点快照取到 connection.host/port → 无法确定（不得判停净）' -Extra @{ provider = 'none' }
    }
    $table = Get-WdTcpEstablished
    if ($null -eq $table) {
        return New-WdItem -State 'indeterminate' -Detail '本机无法枚举 TCP 连接（权限/平台限制；Linux 缺 /proc/net/tcp）→ 无法确定（不得判停净）' -Extra @{ provider = 'unavailable'; device = "$devHost`:$devPort" }
    }
    # 目标地址集合：逐字比对 + 回环等价形式（127.0.0.1 / ::1 / ::ffff:127.0.0.1）
    $wanted = [System.Collections.Generic.HashSet[string]]::new()
    [void]$wanted.Add($devHost)
    if ($devHost -eq 'localhost') { [void]$wanted.Add('127.0.0.1'); [void]$wanted.Add('::1') }
    if ($devHost -eq '127.0.0.1') { [void]$wanted.Add('::1'); [void]$wanted.Add('::ffff:127.0.0.1') }
    if ($devHost -eq '::1') { [void]$wanted.Add('127.0.0.1'); [void]$wanted.Add('::ffff:127.0.0.1') }
    $hits = @()
    $rdp = @()
    foreach ($c in $table) {
        # 3389/RDP：**无论目标端口是多少**都单列并排除，绝不与采集连接混算。
        if ($c.port -eq 3389) { $rdp += $c; continue }
        if ($c.port -eq $devPort -and $wanted.Contains([string]$c.addr)) { $hits += $c }
    }
    $extra = @{
        device        = "$devHost`:$devPort"
        matched       = @($hits | ForEach-Object { "$($_.addr):$($_.port)" })
        excluded_rdp  = @($rdp | ForEach-Object { "$($_.addr):$($_.port)" })
        provider      = $(if ($IsWindows) { 'win-nettcp' } else { 'proc-net-tcp' })
    }
    $rdpNote = if ($rdp.Count -gt 0) { "；已排除 $($rdp.Count) 条 3389/RDP 连接（与采集端口严格区分）" } else { '；无 3389/RDP 连接' }
    if ($hits.Count -gt 0) {
        return New-WdItem -State 'not_converged' -Detail ("到 $devHost`:$devPort 仍有 {0} 条 ESTABLISHED{1}" -f $hits.Count, $rdpNote) -Extra $extra
    }
    return New-WdItem -State 'converged' -Detail ("到 $devHost`:$devPort 无 ESTABLISHED{0}" -f $rdpNote) -Extra $extra
}

$items = [ordered]@{}
$stopHttpOk = $false
$stopHttpAnswered = $false
$stopHttpDetail = '未发出任何 Stop'
foreach ($s in $allStops) {
    if ([int]$s.http_status -gt 0) { $stopHttpAnswered = $true }
    if ([int]$s.http_status -eq 200 -or [int]$s.http_status -eq 500) { $stopHttpOk = $true }
    $stopHttpDetail = "最后一次 Stop：http=$($s.http_status)（200/500 均算停住：500 是 drain 失败但 desired_running 仍落 false，core-api:2181-2190）"
}
# 三态：拿到 200/500 ⇒ 已收敛；一个响应都没拿到（连接失败/超时，http=0）⇒ **无法确定**（不是"没停"）；
# 拿到非 2xx 的明确响应 ⇒ 仍未收敛。
$stopHttpState = if ($stopHttpOk) { 'converged' }
elseif ($allStops.Count -eq 0) { 'not_converged' }
elseif (-not $stopHttpAnswered) { 'indeterminate' }
else { 'not_converged' }
if ($stopHttpState -eq 'indeterminate') { $stopHttpDetail = "$stopHttpDetail → 全部 Stop 请求无 HTTP 响应（连接失败/超时），无法确定端点是否收到 → 判无法确定" }
$items['1_stop_http'] = New-WdItem -State $stopHttpState -Detail $stopHttpDetail -Extra @{}

$finalState = if ($lastValid) { [string]$lastValid.state } else { $null }
$finalDesired = if ($lastValid) { $lastValid.desired_running } else { $null }
$finalEpoch = if ($lastValid) { [string]$lastValid.epoch } else { $null }
# 没有**任何有效样本**时不能用"没看到 = 已停"：那不是 not_converged（观察到仍未停），
# 而是 indeterminate（无法确定，§P0-3 "查询失败/结果不确定一律判无法确定"）。
if (-not $lastValid) {
    $items['2_state_stopped'] = New-WdItem -State 'indeterminate' -Detail '没有任何有效观测样本 → 无法确认 state（不得推断为已停）' -Extra @{}
    $items['3_desired_false'] = New-WdItem -State 'indeterminate' -Detail '没有任何有效观测样本 → 无法确认 desired_running（不得推断为已停）' -Extra @{}
}
else {
    $items['2_state_stopped'] = New-WdItem -State $(if ($finalState -eq 'STOPPED') { 'converged' } else { 'not_converged' }) -Detail "最终观测 state=$finalState" -Extra @{}
    $items['3_desired_false'] = New-WdItem -State $(if ($finalDesired -eq $false) { 'converged' } else { 'not_converged' }) -Detail "最终观测 desired_running=$finalDesired" -Extra @{}
}

# ④⑤：fire 模式**默认强制**（P0-3）。显式 -SkipHostChecks 时记 not_verified —— 永不判停净。
$probeProcErr = $TestHostProbeError -in @('process', 'both')
$probeTcpErr = $TestHostProbeError -in @('tcp', 'both')
if ($SkipHostChecks) {
    $skipItem = {
        param([string]$What)
        $i = New-WdItem -State 'not_verified' -Detail "已显式 -SkipHostChecks：$What 未核对 → 『停干净』未验证（不得判停净）" -Extra @{ provider = 'skipped' }
        $i.required = $false
        return $i
    }
    $items['4_no_driver_process'] = & $skipItem '驱动进程（④）'
    $items['5_no_established_device'] = & $skipItem '到设备的 TCP 连接（⑤）'
}
else {
    $items['4_no_driver_process'] = Test-WdDriverProcess -Ctx $ctx -InjectError $probeProcErr
    $items['5_no_established_device'] = Test-WdDeviceTcp -Ctx $ctx -InjectError $probeTcpErr
}

$settleOk = [bool]$ctx.d['settleOk']
# 没有任何**有效**样本时，"稳定窗口没走完"不能读成"没停稳"——那是无法确定（P0-3 的三态要求）。
$settleState = if ($settleOk) { 'converged' }
elseif (-not $lastValid) { 'indeterminate' }
else { 'not_converged' }
$items['6_settle_10s'] = New-WdItem -State $settleState -Detail $(if ($settleOk) {
        "首次 STOPPED 后连续 $($script:SETTLE_MS / 1000) s 无新 epoch、无 RECONNECTING（窗口内样本 $($ctx.d['settleSamples']) 条，重置 $($ctx.d['settleResets']) 次）"
    }
    elseif ($settleState -eq 'indeterminate') { '没有任何观测样本，稳定窗口无法判定 → 无法确定（不得判停净）' }
    else {
        "稳定窗口未完成（首次 STOPPED mono=$($ctx.d['obsFirstStoppedMono'])，窗口重置 $($ctx.d['settleResets']) 次）"
    }) -Extra @{}

# 三态汇总：not_converged > indeterminate > not_verified > converged
$notConverged = @($items.Keys | Where-Object { $items[$_].required -and $items[$_].state -eq 'not_converged' })
$indeterminate = @($items.Keys | Where-Object { $items[$_].required -and $items[$_].state -eq 'indeterminate' })
$notVerified = @($items.Keys | Where-Object { -not $items[$_].required -and $items[$_].state -eq 'not_verified' })
$convergence = if ($notConverged.Count -gt 0) { 'NOT_CONVERGED' } elseif ($indeterminate.Count -gt 0) { 'INDETERMINATE' } elseif ($notVerified.Count -gt 0) { 'NOT_VERIFIED' } else { 'CONVERGED' }
$mandatoryFail = $notConverged
#endregion

#region ── 判定：调度轴 / 采集轴 / 收敛轴 分开算（F3）──────────────────────────────
$raceRecorded = ($raceCount -gt 0)
$startOutcome = [string]$ctx.d['startOutcome']
$startStatus = [int]$ctx.d['startStatus']
$startUncertain = ($startOutcome -ne 'returned' -or $startStatus -ne 200)
$earlyTriggered = [bool]$ctx.d['earlyTriggered']
$workerAborted = @($ctx.d['workersAborted'])
$workerAbortCount = [int]$ctx.d['workerAbortCount']

# ── 调度轴（"Stop 请求是否准时发出"）：只看截止 Stop 的 |deviation|，不看采集、不看收敛。
$deadlineDevList = @()
$earlyDevList = @()
foreach ($s in $allStops) {
    if ($null -eq $s.deviation_ms) { continue }
    $rs = @($s.reasons)
    if (($rs -contains 'DEADLINE') -or ($rs -contains 'DEADLINE_FAILSAFE')) { $deadlineDevList += [double]$s.deviation_ms }
    else { $earlyDevList += [double]$s.deviation_ms }
}
$maxDev = $null
$maxAbsDev = $null
if ($deadlineDevList.Count -gt 0) {
    $maxDev = ($deadlineDevList | Measure-Object -Maximum).Maximum
    $maxAbsDev = ($deadlineDevList | ForEach-Object { [math]::Abs($_) } | Measure-Object -Maximum).Maximum
}
$deadlineStopPresent = ($deadlineDevList.Count -gt 0)
$scheduleVerdict = if (-not $deadlineStopPresent) { 'UNKNOWN' } elseif ($maxAbsDev -le 1000) { 'PASS' } else { 'FAIL' }

# ── 采集轴（"这一轮的观测证据是否有效"）：与"停住了"完全解耦。
$maxGapMs = [double]$ctx.d['maxSuccessGapMs']
$stallFired = [bool]$ctx.d['obsStallFired']
$truncated = [bool]$ctx.d['obsTruncated']
$obsSamples = [int]$ctx.d['obsSamples']
$samplingReasons = @()
if ($stallFired) { $samplingReasons += "观测心跳失效：MON 判观测通道断流，已提前 Stop（OBS_STALL）" }
if ($maxGapMs -gt [int]$ctx.samplingMaxGapMs) {
    $samplingReasons += ("成功样本最大间隔 {0:N0} ms > {1} ms（窗口内存在观测盲区，S1/S2/S3 可能整段漏判）" -f $maxGapMs, [int]$ctx.samplingMaxGapMs)
}
if ($truncated) { $samplingReasons += "观测总上限 $ObserveMaxSec s 已到（独立计时强制收尾，窗口不完整）" }
if ($obsSamples -eq 0) { $samplingReasons += '没有任何观测样本' }
$samplingVerdict = if ($samplingReasons.Count -eq 0) { 'VALID' }
elseif ($stallFired -or ($maxGapMs -gt [int]$ctx.samplingMaxGapMs)) { 'INVALID_OBSERVATION_GAP' }
elseif ($truncated) { 'INVALID_OBS_TRUNCATED' }
else { 'INVALID_NO_SAMPLES' }
$samplingValid = ($samplingVerdict -eq 'VALID')

# ── 异常项（停住了/未收敛之外的"不能判 PASS"原因）
$anomalies = @()
if (-not $samplingValid) { $anomalies += 'OBS_INVALID' }
if ($startUncertain) { $anomalies += 'START_UNCERTAIN' }
if ($raceRecorded) { $anomalies += 'RACE' }
if ($earlyTriggered) { $anomalies += 'EARLY_TRIGGER' }

if ($workerAbortCount -gt 0) {
    $verdict = 'WORKER_ABORTED'; $exitCode = 8
}
elseif ($convergence -eq 'NOT_CONVERGED') {
    # 已观测到"仍未收敛"（例如仍在 RUNNING / 仍有到 :102 的 ESTABLISHED）——最高严重级。
    $verdict = 'NOT_STOPPED'; $exitCode = 1
}
elseif (-not $samplingValid) {
    # 观测中断是**根因**：没有有效观测时，后续"停没停干净"本就无从确认。
    # 先把它判为"采集无效"，避免被收敛轴的"无法确定"掩盖掉（M8 要的就是这个区分）。
    $verdict = 'STOPPED_OBS_INVALID'; $exitCode = 2
}
elseif ($convergence -eq 'INDETERMINATE') {
    $verdict = 'CONVERGENCE_INDETERMINATE'; $exitCode = 9
}
elseif ($convergence -eq 'NOT_VERIFIED') {
    $verdict = 'HOSTCHECKS_UNVERIFIED'; $exitCode = 7
}
elseif ($anomalies.Count -eq 0) {
    $verdict = 'STOPPED_CLEAN'; $exitCode = 0
}
elseif ($anomalies.Count -gt 1) {
    $verdict = 'STOPPED_WITH_ANOMALIES'; $exitCode = 6
}
else {
    switch ($anomalies[0]) {
        'OBS_INVALID' { $verdict = 'STOPPED_OBS_INVALID'; $exitCode = 2 }
        'START_UNCERTAIN' { $verdict = 'STOPPED_START_UNCERTAIN'; $exitCode = 3 }
        'RACE' { $verdict = 'STOPPED_WITH_RACE'; $exitCode = 4 }
        'EARLY_TRIGGER' { $verdict = 'STOPPED_BY_EARLY_TRIGGER'; $exitCode = 5 }
    }
}

# 试验结论（"这轮证据能不能用于验收"）与退出码分开表达（F3）
$trial = if ($exitCode -eq 0) { 'PASS' }
elseif ($workerAbortCount -gt 0) { 'ABORTED' }
elseif ($convergence -eq 'NOT_CONVERGED') { 'NO_GO' }
elseif (-not $samplingValid) { 'INVALID_SAMPLING' }
elseif ($convergence -in @('INDETERMINATE', 'NOT_VERIFIED')) { 'UNVERIFIED' }
else { 'NO_GO_ANOMALY' }
#endregion

#region ── 汇总输出与落盘 ─────────────────────────────────────────────────────────
# BLOCKED-01：配置门判据（实时任务快照点数）与运行状态观测值（diagnostics.point_count）分开记录。
# 这里取值只读不写，缺失一律 $null（静态阶段未做网络核验时 actual 为空）。
$taskCredItem = $creds.items['task_config']
$taskConfigActual = Get-WdProp $taskCredItem 'actual'

$summary = [ordered]@{
    tag                  = $Tag
    mode                 = $Mode
    pid                  = $PID
    ps_version           = [string]$PSVersionTable.PSVersion
    host                 = $hostName
    base_url             = $ctx.baseUrl
    endpoint_id          = $EndpointId
    script_started_local = $procStart.ToString('o')
    observe_timeout_sec  = $ObserveTimeoutSec
    observe_max_sec      = $ObserveMaxSec
    observe_stall_sec    = $ObserveStallSec
    deadline_offset_sec  = $script:DEADLINE_OFFSET_MS / 1000
    authorization        = [ordered]@{
        overall           = $creds.overall
        authorized        = [bool]$Authorized
        authorized_by     = $AuthorizedBy
        authz_ticket      = $AuthzTicket
        g6_signoff        = $G6Signoff
        g6_signed_at      = $G6SignedAt
        g7_deploy_version = $G7DeployVersion
        g7_binary_path    = $G7BinaryPath
        expected_manifest_sha256 = $ExpectedManifestSha256
        expected_revision = $ExpectedTaskRevision
        # 语义：获批任务配置的点数（BLOCKED-01 起；不是 diagnostics.point_count）。
        expected_task_point_count = $ExpectedPointCount
        approved_task_config = $ApprovedTaskConfig
        expected_tasks_fingerprint = $ExpectedTaskFingerprint
        expected_approved_task_config_sha256 = $ExpectedApprovedTaskConfigSha256
        task_config_ref   = $TaskConfigRef
        failing           = @($creds.failing)
        items             = $creds.items
    }
    boundary             = $boundary
    # ── BLOCKED-01：配置门判据 vs 运行状态观测值（两者语义不同，不得互相顶替）──────────
    #   task_point_count_live            = 实时任务快照 tasks[].binding.config.points[] 总数（**判据**）
    #   diagnostics_point_count_observed = diagnostics.point_count（**只作观测**）：来自持久化
    #     point_registry 行数（core-api:1499 → config-store:2459-2473），唯一写点 assign_point_ids
    #     （config-store:2344-2456，由 run_config_flow:943-945 在 ConfigureTasks 之后写），
    #     因此是"上次走完 ConfigureTasks 的那次 Start"的残留值，与当前任务配置无关。
    task_config_observation = [ordered]@{
        expected_task_point_count        = $ExpectedPointCount
        task_point_count_live            = (Get-WdProp $taskConfigActual 'task_point_count')
        task_count_live                  = (Get-WdProp $taskConfigActual 'task_count')
        diagnostics_point_count_observed = (Get-WdProp $taskConfigActual 'diagnostics_point_count')
        diagnostics_point_count_role     = 'observed-only：运行状态残留值（持久化 point_registry 行数，core-api:1499 ← config-store:2459-2473），非当前任务点数，不参与配置门判定'
        task_config_state                = (Get-WdProp $taskCredItem 'state')
    }
    t0                   = [ordered]@{
        local            = $t0Dto.ToString('o')
        unix_ns          = $t0UnixNs
        mono_ms          = [math]::Round($t0Mono, 1)
        deadline_local   = $deadlineDto.ToString('o')
        deadline_mono_ms = [math]::Round($deadlineMono, 1)
        t0_to_start_ms   = [math]::Round($t0ToStart, 1)
        deadline_note    = 'T0+30 s 是 Stop 请求的截止调度目标；物理终止由 convergence 另行判定'
    }
    start                = [ordered]@{
        outcome    = $startOutcome
        http       = $startStatus
        elapsed_ms = [double]$ctx.d['startElapsedMs']
        dispatch   = [string]$ctx.d['startDispatchLocal']
        err        = [string]$ctx.d['startErr']
        uncertain  = $startUncertain
    }
    stops                     = @($allStops)
    stop_count                = $stopCount
    deadline_stop_present     = $deadlineStopPresent
    deadline_stop_deviation_ms = $maxDev
    max_abs_deviation_ms      = $maxAbsDev
    non_deadline_stop_deviations_ms = @($earlyDevList)
    final                = [ordered]@{
        state           = $finalState
        desired_running = $finalDesired
        epoch           = $finalEpoch
        t_local         = if ($lastValid) { [string]$lastValid.t_local } else { $null }
        t_rel_ms        = if ($lastValid) { $lastValid.t_rel_ms } else { $null }
    }
    six_items            = $items
    mandatory_failures   = @($mandatoryFail)
    convergence          = [ordered]@{
        state        = $convergence
        not_converged = @($notConverged)
        indeterminate = @($indeterminate)
        not_verified  = @($notVerified)
        states       = [ordered]@{}
    }
    race                 = [ordered]@{ count = $raceCount; recorded = $raceRecorded }
    workers              = [ordered]@{
        aborted      = @($workerAborted)
        abort_count  = $workerAbortCount
        mon_events   = @($allMonEvents)
        flows        = @($flows | ForEach-Object { $_.Name })
    }
    observation          = [ordered]@{
        samples         = $obsSamples
        failures        = [int]$ctx.d['obsFailures']
        max_req_ms      = [double]$ctx.d['obsMaxReqMs']
        max_success_gap_ms = $maxGapMs
        stall_fired     = $stallFired
        truncated       = $truncated
        settle_ok       = $settleOk
        settle_samples  = [int]$ctx.d['settleSamples']
        settle_resets   = [int]$ctx.d['settleResets']
        first_stopped   = [double]$ctx.d['obsFirstStoppedMono']
    }
    # ── 三条独立的判定轴：调度（准时）/ 采集（有效）/ 收敛（停净）——评审要求可分别读取 ──
    axes                 = [ordered]@{
        schedule   = [ordered]@{
            verdict            = $scheduleVerdict
            deadline_stop      = $deadlineStopPresent
            max_abs_deviation_ms = $maxAbsDev
            threshold_ms       = 1000
            note               = 'Stop 请求是否在 T0+30 s 截止点准时发出（衡量调度是否被阻塞）'
        }
        sampling   = [ordered]@{
            verdict = $samplingVerdict
            valid   = $samplingValid
            max_gap_ms = $maxGapMs
            threshold_ms = [int]$ctx.samplingMaxGapMs
            reasons = @($samplingReasons)
            note    = '观测证据是否完整（心跳失效/断流/截断一律 INVALID，即使 Stop 准时）'
        }
        convergence = [ordered]@{
            state = $convergence
            note  = '设备/进程是否真的停净；indeterminate = 无法确定，绝不等于已停净'
        }
        start      = $startOutcome
        race       = $(if ($raceRecorded) { "RECORDED x$raceCount" } else { 'NONE' })
        workers    = $(if ($workerAbortCount -gt 0) { "ABORTED: $($workerAborted -join ',')" } else { 'OK' })
    }
    test_hooks           = [ordered]@{
        host_probe_error = $TestHostProbeError
        abort_flow       = $TestAbortFlow
        abort_after_sec  = $(if ($TestAbortFlow) { $TestAbortAfterSec } else { $null })
        # B1：注入参数是否被离线边界守卫放行（正常执行时 active=false）
        injection_guard  = $injectionGuard
    }
    trial                = $trial
    verdict              = $verdict
    exit_code            = $exitCode
    out_dir              = $OutDir
}
foreach ($k in $items.Keys) { $summary.convergence.states[$k] = $items[$k].state }
[System.IO.File]::WriteAllText($summaryPath, ($summary | ConvertTo-Json -Depth 10), [System.Text.UTF8Encoding]::new($false))

Add-WdLog $ctx 'INFO' "判定：$verdict（trial=$trial）；收敛=$convergence；调度=$scheduleVerdict；采集=$samplingVerdict；六项=$((($items.Keys | ForEach-Object { "$_=" + $items[$_].state }) -join ' '))；退出码=$exitCode"

Write-Host ''
Write-Host '================ stop-watchdog 结果 ================'
Write-Host ("PID            : {0}（独立进程）" -f $PID)
Write-Host ("Mode           : {0}（授权：ticket={1}）" -f $Mode, $AuthzTicket)
Write-Host ("BaseUrl        : {0}" -f $ctx.baseUrl)
Write-Host ("Endpoint       : {0}" -f $EndpointId)
Write-Host ("T0             : {0}  (mono {1:N1} ms)" -f $t0Dto.ToString('o'), $t0Mono)
Write-Host ("Deadline       : {0}  (mono {1:N1} ms = T0+{2} s；Stop 请求的截止调度目标，不代表第 30 s 已物理停稳)" -f $deadlineDto.ToString('o'), $deadlineMono, ($script:DEADLINE_OFFSET_MS / 1000))
Write-Host ("T0→Start 发出  : {0:N1} ms" -f $t0ToStart)
Write-Host ("START          : outcome={0} http={1} elapsed={2} ms{3}" -f $startOutcome, $startStatus, $ctx.d['startElapsedMs'], $(if ($startUncertain) { '  ** 结果不确定（在途/超时）→ 按 §5.3 不得判 PASS **' } else { '' }))
if ($allStops.Count -eq 0) {
    Write-Host 'STOP           : ** 未发出任何 Stop **'
}
foreach ($s in $allStops) {
    $d = $s.deviation_ms
    $dText = if ($null -ne $d) { ('{0:+0.0;-0.0;0.0} ms' -f [double]$d) } else { 'N/A' }
    Write-Host ("STOP #{0,-2}       : {1}  reasons={2}  deviation={3}  http={4}  elapsed={5} ms" -f $s.n, $s.t_dispatch_local, (($s.reasons) -join '+'), $dText, $s.http_status, $s.elapsed_ms)
}
if ($null -ne $maxAbsDev) {
    Write-Host ("调度判定        : 截止 Stop 的 |deviation| = {0:N1} ms ⇒ {1}" -f $maxAbsDev, $(if ($maxAbsDev -le 1000) { 'PASS（≤1000 ms，调度未被阻塞）' } else { 'FAIL（>1000 ms）' }))
}
else {
    Write-Host '调度判定        : ** 未发出截止 Stop，无法判定 **'
}
if ($earlyDevList.Count -gt 0) {
    Write-Host ("非截止 Stop    : 相对截止的时标 {0}（提前触发/竞态补发，**不**作为调度判据）" -f (($earlyDevList | ForEach-Object { '{0:+0.0;-0.0;0.0}' -f $_ }) -join ' / '))
}
Write-Host ("最终观测        : state={0} desired_running={1} epoch={2}（t_rel={3} ms）" -f $finalState, $finalDesired, $finalEpoch, $(if ($lastValid) { $lastValid.t_rel_ms } else { 'N/A' }))
Write-Host ("观测            : 样本 {0} 条，失败 {1} 次，单次最长 {2} ms，成功样本最大间隔 {3} ms" -f $obsSamples, $ctx.d['obsFailures'], $ctx.d['obsMaxReqMs'], $maxGapMs)
foreach ($k in $items.Keys) {
    $v = $items[$k]
    $flag = switch ($v.state) {
        'converged' { 'OK   ' }
        'not_converged' { 'FAIL ' }
        'indeterminate' { 'UNKWN' }
        'not_verified' { 'SKIP ' }
        default { '?    ' }
    }
    Write-Host ("  {0,-26} [{1}] {2}" -f $k, $flag, $v.detail)
}
Write-Host ("竞态            : {0}" -f $(if ($raceRecorded) { "记录 RACE_START_AFTER_STOP ×$raceCount（已再次 Stop）" } else { '未观测到' }))
Write-Host ("工作流          : {0}" -f $(if ($workerAbortCount -gt 0) { "异常退出：$($workerAborted -join ',')（MON 已捕获）" } else { '全部正常（MON 监护中）' }))
Write-Host ''
Write-Host ("试验结论        : {0}" -f $trial)
Write-Host ("  调度（准时）  : {0}（截止 Stop |dev| = {1}）" -f $scheduleVerdict, $(if ($null -ne $maxAbsDev) { "$maxAbsDev ms" } else { 'N/A' }))
Write-Host ("  采集（有效）  : {0}{1}" -f $samplingVerdict, $(if ($samplingReasons.Count -gt 0) { ' — ' + ($samplingReasons -join '；') } else { '' }))
Write-Host ("  收敛（停净）  : {0}" -f $convergence)
Write-Host ("判定            : {0}" -f $verdict)
Write-Host ("退出码          : {0}" -f $exitCode)
Write-Host ("证据目录        : {0}" -f $OutDir)

if ($workerAbortCount -gt 0) {
    Write-Host ''
    Write-Host '=========== 工作流异常退出：本轮作废（不得判 PASS）================='
    Write-Host '① 立即人工核实端点真实状态（GET /api/v1/endpoints/{id}）与现场机床状态；'
    Write-Host '② 若端点仍 RUNNING/desired_running=true，按下面第 1–4 步隔离处置；'
    Write-Host '③ 保留本目录全部证据（run-log.md / watchdog-*.jsonl / summary.json），复现命令见 tests/watchdog/README.md；'
    Write-Host ('④ 本次异常工作流：{0}；不得自动重试，须先定位工作流为何退出（改脚本后重跑 M0–M12 全套）。' -f ($workerAborted -join ', '))
}
if ($exitCode -ne 0) {
    Write-Host ''
    Write-Host '=========== 未达成"停干净"：按 §5.3 隔离处置（由监护人执行）==========='
    Write-Host '① 阻断出站：防火墙 Block <机床 IP>:102（对机床零风险，诊断方案 :405）'
    Write-Host '② 杀驱动进程（会触发重连，必须先阻断；:404）'
    Write-Host '③ 杀 mesad（须先确认 desired_running=false；:406）'
    Write-Host '④ 物理隔离只拔**测试主机**网线，不得擅自断产线交换机口（:407 的 E4 影响整线）'
    Write-Host '每一步后复核 state / desired_running / 驱动进程 / 到 :102 的连接，全过程记入 §10.3。'
    Write-Host '不得自动重试、不得自动进入下一轮（§7 统一流程）。'
    if ($exitCode -eq 9) {
        Write-Host '注：本次为"无法确定"（CONVERGENCE_INDETERMINATE）——查询失败/权限不足不等于"没有连接"，必须先修好核对手段再判定。'
    }
    if ($exitCode -eq 7) {
        Write-Host '注：本次显式跳过了 HostChecks（HOSTCHECKS_UNVERIFIED）——不能据此判"停净"，须补做本机核对。'
    }
}
Write-Host '==================================================='

# 收尾：通知所有执行流退出，并释放 runspace。
$ctx.shutdown.Set()
[void]$ctx.wake.Set()
try { [void]$ctx.stopSignal.Release() } catch { }
foreach ($f in $flows) {
    try { $f.PS.Stop() } catch { }
    try { $f.PS.Dispose() } catch { }
    try { $f.RS.Dispose() } catch { }
}
#endregion

exit $exitCode
