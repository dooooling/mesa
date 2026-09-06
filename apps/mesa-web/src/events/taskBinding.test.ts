// PR8 Gate（纯函数）：Web 只生成 mesa.events.v1；私有 binding 只读识别。
import { describe, expect, it } from "vitest";
import {
  GENERIC_EVENT_BINDING_KIND,
  buildGenericEventBinding,
  genericParametersOf,
  genericStreamIdOf,
  isGenericEventTask,
} from "./taskBinding";
import type { EventTask } from "../types";

const generic = (stream_id: string): EventTask => ({
  id: "alarm-main",
  mode: "subscribe",
  interval_ms: null,
  binding: buildGenericEventBinding(stream_id, {}),
});

describe("generic event binding", () => {
  it("唯一构造点生成标准信封", () => {
    const b = buildGenericEventBinding("test.stream.alarm", { area: 1 });
    expect(b.kind).toBe("mesa.events.v1");
    expect(b.config).toEqual({ stream_id: "test.stream.alarm", parameters: { area: 1 } });
    expect(GENERIC_EVENT_BINDING_KIND).toBe("mesa.events.v1");
  });

  it("标准任务识别 + stream/parameters 解析", () => {
    const t = generic("test.stream.counter");
    expect(isGenericEventTask(t)).toBe(true);
    expect(genericStreamIdOf(t)).toBe("test.stream.counter");
    expect(genericParametersOf(t)).toEqual({});
  });

  it("私有 binding 判定为 legacy（不解析、不分支具体 driver）", () => {
    const legacy: EventTask = {
      id: "old",
      mode: "subscribe",
      interval_ms: null,
      binding: { kind: "legacy.private", config: { stream: "test.stream.counter" } },
    };
    expect(isGenericEventTask(legacy)).toBe(false);
    expect(genericStreamIdOf(legacy)).toBeUndefined();
    expect(genericParametersOf(legacy)).toEqual({});
  });
});
