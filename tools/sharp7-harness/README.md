# Sharp7 interop harness（Gate F3 第一阶段，本地复现用）

## 这是什么

用**未修改的上游 Sharp7** 作独立 NCK 客户端，对打 Mesa standalone
emulator，并抓取双方 raw bytes。CI 不跑 dotnet（见下）；CI 只复放
本目录方法生成的 committed vectors
（`drivers/sinumerik-nck/tests/reference/sharp7/`）。

## 上游 pin

- 仓库：`https://github.com/soft79/Sharp7.git`
- commit：`eac1e728f8523278564e83c276fa6b8d281e6ba0`
  （与 `docs/evidence/nck-external-sources.lock.json` 同值）
- `Sharp7.cs` 为该 commit 逐字拷贝，**禁止修改**；
  改动即失去“独立实现”资格，需重新 pin + 重新生成 vectors。

## 再生 vectors（本地，需 dotnet 7+）

```text
# 1. 起 emulator（任一场景；happy 给 GOOD，partial_bad 给按项 BAD）
mesa-nck-emulator --port 1102 --scenario happy --element-size 8

# 2. 跑 harness（Area=N 避开 <<4/<<5 未决分歧；0<<4 == 0<<5）
dotnet run --project tools/sharp7-harness -- \
  --emulator 127.0.0.1:1102 --out drivers/sinumerik-nck/tests/reference/sharp7 \
  --unit 1 --module 18 --param 42 --start 0 --amount 1

# 3. 检查 out 下 req-*.bin / rsp-*.bin / manifest.json，然后跑 Rust 回放测试
cargo test -p mesa-driver-sinumerik-nck --test sharp7_vectors
```

## 已知分歧（ADR 0002，实验时不得绕过）

- area 合成：Sharp7 用 `NckArea<<4`，Mesa/Wireshark/Softing 用 `<<5`。
  本 harness 只用 **Area=0（N）**，两侧字节一致，不触分歧。
- 响应 framing：Sharp7 期望 count@S7[13]/首项@S7[14]
  （疑似 2 字节 errclass/errcode），Mesa 当前 +12。
  emulator 需 `--response-style sharp7` 才可被 Sharp7 成功解码；
  该差异记为 open，不作为任一方“正确”的证据。
- `0x06` 长度单位：Sharp7 按 bit，Wireshark 按 byte；实验避开该 transport。
