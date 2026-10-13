# NCK Browse 合并前修复说明

基线为冻结提交 `80a8c81`，不移动 `nck-v0.5.0-freeze-20261013` 标签。
驱动继续保持experimental；此文只说明软件实现与回归边界。

## Browse 请求范围

管理面为每次Browse创建临时驱动进程，连接内缓存不能跨HTTP请求复用。
因此取消identity_cache，以独立目录和实例游标限定读取范围：

| 操作 | 设备读取范围 |
|---|---|
| 身份根目录及根目录翻页 | 0项；返回轴、候选对象两个目录 |
| 轴列表的一页 | 只读取该页轴，每轴最多5项 |
| 候选对象列表的一页 | 只读取该页编号，每对象1项p0199 |
| 展开一个轴或其变量翻页 | 只复核该轴，最多5项 |
| 展开一个候选对象或其变量翻页 | 只复核该对象，1项 |

轴列表和对象列表的游标为下一个实例编号，范围1..31；根目录和变量页
继续使用本地偏移游标。过滤只作用于当前已读页，无匹配时仍返回下一实例
游标；错误诊断不受过滤影响。保留整次请求15秒时限。

两个入口独立展示。NC轴变量从机床数据的驱动号绑定；候选对象变量从本页
对象编号绑定。使用候选入口前按p0199名称核对，不再通过每次全量扫描
自动配对，也不根据相同编号假定对象身份。

## 设备拒绝与表尾

- 成功解码的空名称：正常表尾。
- 0x8201及其他作业拒绝：诊断保留S7_JOB原始码，明确说明后续未枚举。
- 单项拒绝：诊断保留S7_ITEM原始码。
- 响应长度、类型、解码或通信异常：返回错误，不转成空目录。
- 已读页保留；某轴的NC对象名被拒绝时，该轴保留拒绝说明，不生成未确认绑定。

## 回归验证

`tests/identity_browse.rs`使用独立记录的原始响应，每次调用都重建
NckConnection，模拟正式Browse的临时连接生命周期。6项回归验证请求数量、
分页起点、入口绑定以及拒绝/空名称/畸形响应的区分。

合并前执行：

```text
cargo fmt --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked -p mesa-driver-sinumerik-nck -p mesa-s7-transport
cargo test --locked -p mesa-driver-s7 -p mesa-nck-emulator --all-features
python scripts/write-contract-evidence.py
```

公共S7 Transport中已有的PLC读写、Setup/Read/Write/SZL分段响应测试应继续
通过。正式PR的最终SHA仍须通过GitHub ci-gate；本地记录不替代该结果。
已完成的实体828D240点记录属于冻结基线的正向证据，本次Browse修复不
宣称重新完成硬件Gate或支持全部机型。
