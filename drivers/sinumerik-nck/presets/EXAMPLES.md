# 828D 原生寻址配置示例

每条示例都是**可直接提交的 `ResourceSelection.parameters`**（已含使用者确认的
`area_no`/`line`）。`wire` 与 `source_label` 由 `tests/examples_828d.rs` 逐条复核，
文档不会与代码漂移。文本块内容与 `examples-828d.json` 同源生成。

> 提醒：**协议读取成功不等于工程量已确认**。示例里的单位与语义来自候选定义，
> 不等于该机床已验证。手工地址默认没有 `r0035=-200` 这类保护，必须显式配置。

## 1. 普通通道 NC 变量：程序状态（U16 小端 → Core U32）

```json
{
  "area": "C",
  "module": 127,
  "column": 13,
  "count": 1,
  "unit_mode": "current",
  "decode": "U16_LE",
  "transport_size": "0x09",
  "area_no": 1,
  "line": 1
}
```

- 请求字节：`12 08 82 41 00 0d 00 01 7f 01`
- 地址标识：`nck://read/C/1/127/13/1/1?mode=current&decode=U16_LE&transport=0x09`

## 2. 通道进给速度（F64 小端；line=1 为该通道自身实例）

```json
{
  "area": "C",
  "module": 127,
  "column": 1,
  "count": 1,
  "unit_mode": "current",
  "decode": "F64_LE",
  "transport_size": "0x09",
  "area_no": 1,
  "line": 1
}
```

- 请求字节：`12 08 82 41 00 01 00 01 7f 01`
- 地址标识：`nck://read/C/1/127/1/1/1?mode=current&decode=F64_LE&transport=0x09`

## 3. 当前程序名（32 字节 NUL 文本 → Core String）

```json
{
  "area": "C",
  "module": 125,
  "column": 3,
  "count": 1,
  "unit_mode": "current",
  "decode": "UTF8_NUL_LAST_32",
  "transport_size": "0x09",
  "area_no": 1,
  "line": 1
}
```

- 请求字节：`12 08 82 41 00 03 00 01 7d 01`
- 地址标识：`nck://read/C/1/125/3/1/1?mode=current&decode=UTF8_NUL_LAST_32&transport=0x09`

## 4. 当前执行程序块（66 字节 NUL 文本 → Core String）

```json
{
  "area": "C",
  "module": 125,
  "column": 31,
  "count": 1,
  "unit_mode": "current",
  "decode": "UTF8_NUL_FIRST_66",
  "transport_size": "0x09",
  "area_no": 1,
  "line": 1
}
```

- 请求字节：`12 08 82 41 00 1f 00 01 7d 01`
- 地址标识：`nck://read/C/1/125/31/1/1?mode=current&decode=UTF8_NUL_FIRST_66&transport=0x09`

## 5. 轴变量：line=机床轴索引（MCS 轴设定位置；此处 axis3=MZ1）

```json
{
  "area": "C",
  "module": 115,
  "column": 2,
  "count": 1,
  "unit_mode": "current",
  "decode": "F64_LE",
  "transport_size": "0x09",
  "area_no": 1,
  "line": 3
}
```

- 请求字节：`12 08 82 41 00 02 00 03 73 01`
- 地址标识：`nck://read/C/1/115/2/3/1?mode=current&decode=F64_LE&transport=0x09`

## 6. 轴变量：line=机床轴索引（BCS 实际位置）

```json
{
  "area": "C",
  "module": 112,
  "column": 33,
  "count": 1,
  "unit_mode": "current",
  "decode": "F64_LE",
  "transport_size": "0x09",
  "area_no": 1,
  "line": 3
}
```

- 请求字节：`12 08 82 41 00 21 00 03 70 01`
- 地址标识：`nck://read/C/1/112/33/3/1?mode=current&decode=F64_LE&transport=0x09`

## 7. R 参数：line=R 参数号+1（此处 R425）

```json
{
  "area": "C",
  "module": 21,
  "column": 1,
  "count": 1,
  "unit_mode": "current",
  "decode": "F64_LE",
  "transport_size": "0x09",
  "area_no": 1,
  "line": 426
}
```

- 请求字节：`12 08 82 41 00 01 01 aa 15 01`
- 地址标识：`nck://read/C/1/21/1/426/1?mode=current&decode=F64_LE&transport=0x09`

## 8. 主轴变量：line=主轴索引

```json
{
  "area": "C",
  "module": 114,
  "column": 3,
  "count": 1,
  "unit_mode": "current",
  "decode": "F64_LE",
  "transport_size": "0x09",
  "area_no": 1,
  "line": 2
}
```

- 请求字节：`12 08 82 41 00 03 00 02 72 01`
- 地址标识：`nck://read/C/1/114/3/2/1?mode=current&decode=F64_LE&transport=0x09`

## 9. SINAMICS 只读驱动参数：滤波电流实际值（module 26 = NC 变量入口）

```json
{
  "area": "V",
  "module": 26,
  "column": 27,
  "count": 1,
  "unit_mode": "current",
  "decode": "F32_BE",
  "transport_size": "0x07",
  "area_no": 1,
  "line": 1
}
```

- 请求字节：`12 08 82 a1 00 1b 00 01 1a 01`
- 地址标识：`nck://read/V/1/26/27/1/1?mode=current&decode=F32_BE&transport=0x07`

## 10. 电机温度：无效温度哨兵 `-200` 按 BAD（模板已预填该规则，手填地址须自行补上）

```json
{
  "area": "V",
  "module": 26,
  "column": 35,
  "count": 1,
  "unit_mode": "current",
  "decode": "F32_BE",
  "transport_size": "0x07",
  "area_no": 3,
  "line": 1,
  "invalid_value": -200.0,
  "invalid_tolerance": 0.001
}
```

- 请求字节：`12 08 82 a3 00 23 00 01 1a 01`
- 地址标识：`nck://read/V/3/26/35/1/1?mode=current&decode=F32_BE&transport=0x07`

## 11. 同上，但走显式驱动对象入口（module 130）——两条入口编号不同，不得互相换算

```json
{
  "area": "V",
  "module": 130,
  "column": 35,
  "count": 1,
  "unit_mode": "current",
  "decode": "F32_BE",
  "transport_size": "0x07",
  "area_no": 3,
  "line": 1,
  "invalid_value": -200.0,
  "invalid_tolerance": 0.001
}
```

- 请求字节：`12 08 82 a3 00 23 00 01 82 01`
- 地址标识：`nck://read/V/3/130/35/1/1?mode=current&decode=F32_BE&transport=0x07`

## 12. 驱动对象名（25×U16BE=50 字节 → 一个 UTF-16 字符串）

```json
{
  "area": "V",
  "module": 26,
  "column": 199,
  "count": 25,
  "unit_mode": "current",
  "decode": "UTF16_BE",
  "transport_size": "0x06",
  "area_no": 1,
  "line": 1
}
```

- 请求字节：`12 08 82 a1 00 c7 00 01 1a 19`
- 地址标识：`nck://read/V/1/26/199/1/25?mode=current&decode=UTF16_BE&transport=0x06`

## 13. 驱动对象名（显式驱动对象入口 module 130；按名称核对身份）

```json
{
  "area": "V",
  "module": 130,
  "column": 199,
  "count": 25,
  "unit_mode": "current",
  "decode": "UTF16_BE",
  "transport_size": "0x06",
  "area_no": 1,
  "line": 1
}
```

- 请求字节：`12 08 82 a1 00 c7 00 01 82 19`
- 地址标识：`nck://read/V/1/130/199/1/25?mode=current&decode=UTF16_BE&transport=0x06`

## 身份核对（内建探针，不是采集地址）

轴名与驱动对象名由驱动内建的 `IdentityProbeSpec` 提供，见 `nck://identity`；
探针参数（module/column/decode/transport）内建在 `src/identity_spec.rs`，
不随模板文件变化。已核对的 828D 探针：轴名表、`MDCA_CTRLOUT_MODULE_NR`、
`MDCA_CTRLOUT_TYPE`、`MDCA_ENC_MODULE_NR`、`SPIND_ASSIGN_TO_MACHAX`、
`AXCONF_MACHAX_USED`、`p0199`（两条入口各一）。
