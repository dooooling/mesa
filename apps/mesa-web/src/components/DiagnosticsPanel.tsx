// 通用 DiagnosticsPanel（V2.1 §9）
import { useEffect, useState } from "react";
import { api } from "../api";

export function DiagnosticsPanel() {
  const [data, setData] = useState<unknown>(null);
  useEffect(() => { api.diagnostics().then(setData).catch(() => {}); }, []);
  return <pre style={{ background: "#f5f5f5", padding: 12 }}>{JSON.stringify(data, null, 2)}</pre>;
}
