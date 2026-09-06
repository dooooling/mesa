// StoredEvent 测试夹具（与 core-api stored_event_json 形态一致）。
import type { StoredEvent } from "../types";

type EventOverrides = Partial<Omit<StoredEvent, "event">> & { event?: Partial<StoredEvent["event"]> };

const BASE_EVENT: StoredEvent["event"] = {
  event_id: "EV-0",
  category: "alarm",
  kind: "alarm.condition",
  source: "Channel1",
  severity: 700,
  code: "700012",
  message: "overtemp",
  message_locale: "en",
  occurred_at_ns: 1_700_000_000_000_000_000,
  published_at_ns: 1_700_000_000_100_000_000,
  connection_handle: 1,
  condition: {
    condition_id: "SIM-ALARM-100",
    transition: "raised",
    active: true,
    acknowledged: null,
    confirmed: null,
    retain: true,
  },
  correlation_id: null,
  attributes: { axis: 1 },
};

export function makeEvent(seq: number, overrides?: EventOverrides): StoredEvent {
  const { event: eventOverrides, ...rest } = overrides ?? {};
  return {
    seq,
    endpoint_id: "sim-01",
    stream_epoch: 3,
    batch_sequence: seq,
    received_at_ns: 1_700_000_000_000_000_000 + seq * 1_000_000,
    ...rest,
    event: {
      ...BASE_EVENT,
      event_id: `EV-${seq}`,
      occurred_at_ns: 1_700_000_000_000_000_000 + seq * 1_000_000,
      published_at_ns: 1_700_000_000_100_000_000 + seq * 1_000_000,
      ...eventOverrides,
    },
  };
}
