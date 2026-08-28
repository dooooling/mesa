# Simulator Driver（标杆）

- **绑定** `simulator.points` `poll` `interval_ms + burst`
- **数据源** Constant/Counter/Sine/Toggle/Random（附录 A.1 子集），`TODO: delay/jitter/silent_interval 待 §22 补齐`
- **质量** `quality BAD/UNCERTAIN` `bad_after_batches/good_again_after` 转换，`faults fail_after_batches/crash_after_batches`
- **Contract** 唯一全过 §21 23项 的基线，S7/FOCAS/OPC UA 不得绕过；`Soak 50K burst125`
