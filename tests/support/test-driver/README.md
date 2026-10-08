# mesa-test-driver（测试基础设施，非正式设备协议）

前身为 `drivers/simulator`。退役产品身份后迁移至此：不再出现在正式
`drivers/` discovery 目录；`driver_id` 固定为 `test-driver`。

- **绑定**（Foundation-2 单路径）`mesa.resources.v1` + `TaskSchedule::Poll{interval_ms}`（`burst` 为 binding 顶层扩展参数）
- **数据源** Constant/Counter/Sine/Toggle/Random（附录 A.1 子集），`TODO: delay/jitter/silent_interval 待 §22 补齐`
- **质量** `quality BAD/UNCERTAIN` `bad_after_batches/good_again_after` 转换，`faults fail_after_batches/crash_after_batches`
- **Contract** 唯一全过 §21 23项 的基线，S7/FOCAS/OPC UA 不得绕过；`Soak 50K burst125`
