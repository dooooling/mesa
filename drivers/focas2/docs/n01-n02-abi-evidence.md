# N01 / N02 ABI 证据（PR #59 合并验收用）

> 来源：仓库内 `drivers/focas2/libs/win/` 自带 DLL 的本地反编译审查材料。
> 本文件只收录支撑以下两个生产改动所需的**最小关键片段**，
> 不提交整套反编译工程；reviewer 无需访问外部未追踪目录即可独立审计。

- `FWLIB64.dll`（Data Window Library for x64，`FileVersion 7.3.0.1`）
  `SHA256 7B3837E3925902D1E06C5C7E4F9CE39FE7F39AA9CAED029096187F5641EC04AB`
- `fwlibe64.dll`（Data Window Library for Ethernet(x64)，`FileVersion 5.3.0.1`）
  `SHA256 D3341B43BF3945BDB49D03D1D96436A75911BF3C8D1CDF4F30B9167EF935BFD6`

两 DLL 的 `cnc_rdparam` 入口签名均为 5 参
`(handle, number, axis/type, length, output_pointer)`，
`cnc_rdtofs` 入口签名均为 5 参
`(handle, number, type, length, output_pointer)`；
与之前误写的 6 参 `(hdl, s_no, type, e_no, length, out)` 不符（见 §2）。

## 1. N01：`length` 是缓冲区容量，类型由参数属性决定

### 1.1 入口与内部分发（`fwlibe64.dll`）

```text
// fwlibe64.dll 反编译（RVA 0x253f0）
__int64 __fastcall cnc_rdparam(
    unsigned __int16 a1,   // handle
    __int16 a2,            // number（参数号）
    unsigned __int16 a3,   // axis（轴号，0=无轴）
    unsigned __int16 a4,   // length（缓冲区容量，字节数）
    _WORD *a5)             // output（IODBPSD_1*，2+2+4=8B）
{
  return sub_180024CD8(a1, a2, a3, a4, a5, 0xEu, 0);
}
```

`a4` 未经任何“类型选择”解释，直接作为容量 `a4` 传入内部函数；
真正的属性查询在内部函数头部（`cnc_rdparainfo`），见 1.2。

### 1.2 内部先查属性（`sub_180024CD8` 头部）

```text
// fwlibe64.dll 反编译（RVA 0x24cd8），错误路径字符串原样保留：
//   "Error: cnc_rdparainfo(" ... ") in cnc_rdparam"
sub_180064F38(/*...*/, 0xA0u, a2, 1u, /*...*/);   // 0xA0 = parainfo 命令
// ...
v15 = sub_1800444A0(/*...*/);
if (ntohs(v15[7]) != 28) { /* throw EW_ATTRIB(3)... */ }
// v45 = ntohs(v15[15]) == 2;   // 属性：REAL 标记
```

即 DLL 在读写数值之前，先按参数号查询属性（parainfo），
`length` 不参与类型选择，只参与后面的容量门。

### 1.3 容量门（单值判别值 5 / 6 / 8）

```text
v19 = a4;                       // v19 = 传入 length（容量）
if (v19 < 4) { throw Length(2); }// 总长 < 4 直接 EW_LENGTH
// *a5 = datano 回写；a5[1] = 属性/轴号回写
v24 = (unsigned __int8)ntohs(v18[11]);   // v24 = 属性宽度码
// ...
if (v24 <= 1u) {                        // byte 分支
  if (v19 < v23 + 4) { throw Length(2); }
  // 每值写 1B：*((_BYTE *)a5 + v9++ + 4) = ntohl(v27);
} else switch (v24) {
  case 2u:                              // word 分支
    if (v19 < 2 * v23 + 4) { throw Length(2); }
    // 每值写 1×u16：a5[v9++ + 2] = ntohl(v26);
    break;
  case 3u:                              // dword 分支
    if (v19 < 4 * v23 + 4) { throw Length(2); }
    // 每值写 1×u32：*(_DWORD *)&a5[2 * v9++ + 2] = ntohl(v25);
    break;
  case 4u:                              // REAL 分支（8B：u32 + i16）
    if (v19 < 8 * v23 + 4) { throw Length(2); }
    // *(_DWORD *)&a5[4*v9+2] = ...; *(_WORD *)&a5[4*v9+4] = ...;
    break;
}
```

单值（`v23 = 1`，`cnc_rdparam` 单点语义）代入：

```text
byte:  len ≥ 1+4 = 5
word:  len ≥ 2+4 = 6
dword: len ≥ 4+4 = 8
REAL:  len ≥ 8+4 = 12
```

生产 `PSD1_PROBES = [5, 6, 8]` 即此判别值，按容量递增试、
首次成功即判别宽度（小容量已排除窄类型）。
旧序列 `1/8/6` 错误：`len<4` 恒 `EW_LENGTH` 无判别力；
`len=6` 对 dword 容量不足，不能当 dword 探测点；
`len=8` 三者都够，不能反推为 WORD（此前 DWORD=65536 被截成 0 的根因）。

适用边界：以上是**单值**容量门；DLL 存在多值分支
（`v23>1` 时容量门按 `v23` 缩放）。生产 `cnc_rdparam` 只读单值
（`IODBPSD_1` 单点语义），不推广为所有参数类型已验证；
尚无真机全宽度对照（见 PR body 验收边界）。

## 2. N02：`cnc_rdtofs` 是 5 参，`a4` 为 length

### 2.1 入口签名（两 DLL 一致，均为 5 参）

```text
// FWLIB64.dll / fwlibe64.dll 反编译
//   cnc_rdtofs(unsigned __int16 a1, __int16 a2, __int16 a3,
//              __int16 a4, __int64 a5)
//   a1=handle, a2=number, a3=type, a4=length, a5=output(ODBTOFS*)
```

不是范围接口 `cnc_rdtofsr`，没有 `e_no` 参数。
之前误写的 6 参 `(hdl, num, t, num, 8, &out)` 会把整数 `8` 当输出指针，
已回滚（B1 前即修正，复测确认保持正确）。

### 2.2 关键语义（`fwlibe64.dll`，RVA 0x33f6c）

```c
__int64 __fastcall cnc_rdtofs(
    unsigned __int16 a1, unsigned __int16 a2, __int16 a3,
    unsigned __int16 a4, __int64 a5)
{
  if (a4 < 8u)
    return 2;   // length < 8 → EW_LENGTH
  result = cnc_rdtofsr(a1, a2, a3 + 1000, a2, 0xBCu, v9);
  if (!(_WORD)result) {
    *(_WORD *)a5 = a2;          // out.datano = number
    *(_WORD *)(a5 + 2) = a3;    // out.type = type
    *(_DWORD *)(a5 + 4) = v8;   // out.data
  }
  return result;
}
```

- `a4` 是 `length`：`a4 < 8 → EW_LENGTH(2)`；
- `a3` 是 `type`：透传时 `a3+1000` 转发给 `cnc_rdtofsr`；
- 生产调用 `(hdl, num_s, t, 8, &out)`（`t ∈ {0,1}` 先试几何/磨损，
  `length=8` = `ODBTOFS` Pack=4 8B，零初始化输出）与上完全一致。
- 旧调用 `(num, num, t)` 把 type 值放在 length 槽，
  `t=0/1 < 8` 恒 `EW_LENGTH`，直接路径恒失败后靠回退掩盖。

## 3. 与生产代码的对应关系

| 生产位置 | 证据段 |
|---|---|
| `native.rs PSD1_PROBES = [5,6,8]`、容量递增判别、`datano` 回显门 | §1.3 |
| `native.rs` REAL 精确门（整除 + `i32` 范围，否则 `Data`） | §1.3 `case 4u`（REAL 8B 独立分支，不与整数混读） |
| `native.rs FnRdTofs` 5 参类型别名、`cnc_rdtofs(hdl,num,t,8,out)` | §2.1–§2.2 |
| `native.rs` 零初始化输出缓冲（DLL 只写有效宽度） | §1.3 各分支“每值写 1/2/4B”，其余字节 DLL 不写 |
