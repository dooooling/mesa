// V2.1 通用类型（与 core-types 镜像，不含协议语义）
export type FieldType =
  | "string"
  | "integer"
  | "number"
  | "boolean"
  | "enum"
  | "secret"
  | "duration"
  | "host"
  | "port"
  | "url"
  | "file"
  | "certificate_ref";

export interface UiHints {
  group?: string;
  order?: number;
  placeholder?: string;
  advanced?: boolean;
  visible_if?: { field: string; op: "eq" | "neq" | "in"; value: unknown };
}

export interface FieldValidation {
  min?: number;
  max?: number;
  pattern?: string;
  enum_options?: string[];
}

export interface FieldDescriptor {
  key: string;
  label: string;
  description?: string;
  field_type: FieldType;
  required: boolean;
  default?: unknown;
  validation: FieldValidation;
  ui: UiHints;
}

export interface SchemaDescriptor {
  fields: FieldDescriptor[];
}

export interface LocalizedText {
  default: string;
  "zh-CN"?: string;
}

// 与 core-types DataType 的 serde wire format 镜像（PascalCase；
// as_str() 的小写形态不是 wire format，不得用于此类型）。
// 若 Rust 侧改 wire，descriptor_contract 的 wire 断言会先红。
export type DataType =
  | "Bool" | "I32" | "U32" | "I64" | "U64" | "F32" | "F64"
  | "String" | "Bytes" | "DateTime"
  | "BoolArray" | "I32Array" | "U32Array" | "I64Array" | "U64Array"
  | "F32Array" | "F64Array" | "StringArray" | "DateTimeArray";

export type OutputTypeSpec =
  | { kind: "fixed"; data_type: DataType }
  | { kind: "from_parameter"; parameter: string; mapping: Record<string, DataType> }
  | { kind: "driver_resolved" };

export interface OutputDescriptor {
  id: string;
  label: LocalizedText;
  type_spec: OutputTypeSpec;
  unit?: string;
  access: "read" | "write" | "readwrite";
}

export interface ResourceDescriptor {
  id: string;
  label: LocalizedText;
  parameters: SchemaDescriptor;
  outputs: OutputDescriptor[];
  modes: string[];
}

export interface DriverDescriptor {
  contract_major: number;
  contract_minor: number;
  identity: { driver_id: string; name: string; version: string };
  connection: SchemaDescriptor;
  resources: ResourceDescriptor[];
  controls: { commands: unknown[] };
  resource_selection_methods: Array<"manual" | "browse" | "import">;
  capabilities: { poll: boolean; subscribe: boolean; write: boolean; method: boolean; events?: boolean };
  // Event Plane V1 §5：老 Driver 可能缺省该字段，Web 按空目录处理（与 Rust serde(default) 对齐）
  events?: EventCatalog;
}

// PR8 通用事件契约（与 core-types/src/event.rs 镜像，不含协议语义）
export type TaskMode = "poll" | "subscribe";

export interface EventFieldDescriptor {
  key: string;
  label: LocalizedText;
  data_type?: string;
}

export interface EventStreamDescriptor {
  id: string;
  label: LocalizedText;
  modes: TaskMode[];
  parameters: SchemaDescriptor;
  fields: EventFieldDescriptor[];
}

export interface EventCatalog {
  streams: EventStreamDescriptor[];
}

export interface DriverBinding {
  kind: string;
  config: unknown;
}

export interface EventTask {
  id: string;
  mode: TaskMode;
  interval_ms?: number | null;
  binding: DriverBinding;
}

export type ConditionTransition = "raised" | "updated" | "acknowledged" | "confirmed" | "cleared";

export interface StoredEventCondition {
  condition_id: string;
  transition: ConditionTransition;
  active?: boolean | null;
  acknowledged?: boolean | null;
  confirmed?: boolean | null;
  retain?: boolean | null;
}

// PR7 GET /api/v1/events 响应行：StoredEvent → event 嵌套形态（core-api stored_event_json）
export interface StoredEvent {
  seq: number;
  endpoint_id: string;
  stream_epoch: number;
  batch_sequence: number;
  received_at_ns: number;
  event: {
    event_id: string;
    category: string;
    kind: string;
    source: string;
    severity: number;
    code?: string | null;
    message?: string | null;
    message_locale?: string | null;
    occurred_at_ns?: number | null;
    published_at_ns: number;
    connection_handle: number;
    condition?: StoredEventCondition | null;
    correlation_id?: string | null;
    attributes: Record<string, unknown>;
  };
}

export interface ListEventsResponse {
  events: StoredEvent[];
  next_cursor: number | null;
}

export interface EventFilter {
  endpoint_id?: string;
  category?: string;
  kind?: string;
  severity_min?: number;
  code?: string;
  condition_id?: string;
  active?: boolean;
  from_ns?: number;
  to_ns?: number;
  before_seq?: number;
  after_seq?: number;
  limit?: number;
}

export interface EventStats {
  sse_lagged_total: number;
  sse_replay_frames_total: number;
  sse_reconcile_total: number;
  ingress_batches_total: number;
  ingress_persisted_events_total: number;
  ingress_batch_duplicates_total: number;
  ingress_event_duplicates_total: number;
  ingress_gaps_total: number;
  ingress_regressions_total: number;
  ingress_collisions_total: number;
  ingress_invalid_total: number;
  ingress_store_failures_total: number;
  retention_purged_total: number;
  live_clients: number;
  stored_rows: number;
  stored_size_bytes: number;
}

export interface ValidationIssue {
  path: string;
  code: string;
  message: string;
}
