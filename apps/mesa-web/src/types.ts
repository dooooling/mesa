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

export interface OutputDescriptor {
  id: string;
  label: LocalizedText;
  data_type: string;
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
  discovery: { manual: boolean; browse: boolean; import: boolean };
  capabilities: { poll: boolean; subscribe: boolean; browse: boolean; write: boolean; method: boolean };
}

export interface ValidationIssue {
  path: string;
  code: string;
  message: string;
}
